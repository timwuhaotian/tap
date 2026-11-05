//! Where passwords live (Search's `Vault.swift`): the macOS keychain, under Tap's own
//! label, as internet passwords keyed by site and account. Nothing is written to disk in
//! any other form, and nothing is ever logged.

use std::{
    collections::BTreeSet,
    ffi::{CStr, c_void},
    sync::OnceLock,
};

use objc2_foundation::NSString;
use serde_json::Value;
use tap_core::{identity, settings_index::Prefs as _};

use crate::{keychain, prefs};

/// One kept account as a list needs it: no secret. Reading a secret is one keychain call,
/// asked only for the account picked.
#[derive(Debug, Clone, PartialEq)]
pub struct Kept {
    pub host: String,
    pub user: String,
    /// When it was last used to sign in, seconds since 1970. Newest first in lists.
    pub used: Option<f64>,
    /// Kept from a page sent over plain http: offered only on such a page.
    pub clear: bool,
}

/// One account with its secret, for the paths that hand a password over.
pub struct Login {
    pub host: String,
    pub user: String,
    pub password: String,
    pub clear: bool,
}

const NEVER: &str = "passwords.never";

fn world() -> Option<String> {
    identity::world(std::env::var(identity::PROBE_ENV).ok().as_deref())
}

/// What every item of Tap's is tagged with. A test run tags its own, so a password saved
/// while trying something never sits among the real ones.
fn label() -> String {
    identity::folder_name(world().as_deref())
}

/// Tap's drawer: its label, and a security domain of its own, which keeps its items
/// apart from Search's for the same accounts (`keychain::Drawer`).
fn ours(label: &str) -> keychain::Drawer<'_> {
    keychain::Drawer {
        label,
        domain: Some(label),
    }
}

fn kept_from(row: keychain::Row) -> Kept {
    Kept {
        host: row.server,
        user: row.account,
        used: row.comment.and_then(|c| c.parse().ok()),
        clear: row.clear,
    }
}

/// What Tap keeps, narrowed to `host` if given.
fn listed(host: Option<&str>) -> Vec<Kept> {
    keychain::rows(&label(), host)
        .into_iter()
        .map(kept_from)
        .collect()
}

/// What is kept for a host, exactly.
pub fn kept_for(host: &str) -> Vec<Kept> {
    listed(Some(host))
}

/// The accounts offered on `host`: the host's own, then with `wide` those of every host
/// sharing its registrable domain (accounts.example.com asks, the password was kept for
/// example.com). Newest first.
pub fn kept_matching(host: &str, wide: bool) -> Vec<Kept> {
    let domain = registrable(host);
    let mut out = kept_for(host);
    if wide {
        out.extend(
            listed(None)
                .into_iter()
                .filter(|k| k.host != host && belongs(&k.host, host, &domain)),
        );
    }
    out.sort_by(|a, b| b.used.unwrap_or(0.0).total_cmp(&a.used.unwrap_or(0.0)));
    out
}

/// Every account Tap keeps: for counting.
pub fn all() -> Vec<Kept> {
    listed(None)
}

/// Whether an account kept for `server` is offered on `host`: the same host, or another
/// host of the same site. Never across an address or an unknown domain.
fn belongs(server: &str, host: &str, domain: &str) -> bool {
    server == host || (domain.contains('.') && registrable(server) == domain)
}

/// One account's secret, read off the main thread.
pub fn login(kept: &Kept) -> Option<Login> {
    let password = keychain::secret(ours(&label()), &kept.host, &kept.user)?;
    Some(Login {
        host: kept.host.clone(),
        user: kept.user.clone(),
        password,
        clear: kept.clear,
    })
}

/// Keeps a password in Tap's drawer, updated if it is there already.
pub fn save(host: &str, user: &str, password: &str, last: Option<f64>, clear: bool) -> bool {
    keychain::save(ours(&label()), host, user, password, last, clear)
}

/// It was just used to sign in: lists put it first from now on.
pub fn touch(login: &Login) {
    save(
        &login.host,
        &login.user,
        &login.password,
        Some(tap_core::history::now()),
        login.clear,
    );
}

/// Takes one account out of Tap's drawer.
pub fn forget(host: &str, user: &str) -> bool {
    keychain::forget(ours(&label()), host, user)
}

// MARK: - sites that asked not to be asked

/// The sites told never to offer saving.
pub fn never() -> BTreeSet<String> {
    match prefs::Kept::new().get(NEVER) {
        Some(Value::Array(list)) => list
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        _ => BTreeSet::new(),
    }
}

pub fn set_never(sites: &BTreeSet<String>) {
    let list = sites.iter().cloned().map(Value::String).collect();
    let mut kept = prefs::Kept::new();
    if sites.is_empty() {
        kept.set(NEVER, Value::Null);
    } else {
        kept.set(NEVER, Value::Array(list));
    }
}

pub fn add_never<I: IntoIterator<Item = String>>(hosts: I) {
    let mut sites = never();
    sites.extend(hosts);
    set_never(&sites);
}

pub fn is_never(host: &str) -> bool {
    let sites = never();
    sites.contains(host) || sites.contains(&registrable(host))
}

/// What another browser's store gave, put in Tap's drawer, with the sites it was told
/// never to ask about (Search's `keep`). Saving one already kept updates it, so bringing
/// the same in again adds nothing twice. Answers how many were kept.
pub fn keep(logins: &[tap_import::passwords::Login], never_sites: Vec<String>) -> usize {
    let kept = logins
        .iter()
        .filter(|l| save(&l.host, &l.user, &l.password, l.used, l.clear))
        .count();
    add_never(never_sites);
    kept
}

// MARK: - the site behind a host

type IsSuffix = unsafe extern "C" fn(*const NSString) -> bool;

/// WebKit's own test for a public suffix, from the list macOS keeps (Search's
/// `Passkeys.publicSuffix`). Private to `CFNetwork`; without it a host is only ever its own
/// site, which errs toward offering less.
fn is_suffix() -> Option<IsSuffix> {
    static FOUND: OnceLock<Option<usize>> = OnceLock::new();
    let found = (*FOUND.get_or_init(|| {
        let path = c"/System/Library/Frameworks/CFNetwork.framework/CFNetwork";
        let name: &CStr = c"_CFHostIsDomainTopLevel";
        // SAFETY: dlopen of a system framework already loaded into the process, and a
        // plain symbol lookup in it.
        let symbol: *mut c_void = unsafe {
            let handle = libc::dlopen(path.as_ptr(), libc::RTLD_NOW);
            if handle.is_null() {
                return None;
            }
            libc::dlsym(handle, name.as_ptr())
        };
        (!symbol.is_null()).then_some(symbol as usize)
    }))?;
    // SAFETY: the symbol is CFNetwork's `Boolean _CFHostIsDomainTopLevel(CFStringRef)`.
    Some(unsafe { std::mem::transmute::<usize, IsSuffix>(found) })
}

fn suffix(name: &str) -> bool {
    let Some(test) = is_suffix() else {
        return false;
    };
    let name = NSString::from_str(name);
    // SAFETY: a valid string for the call's length.
    unsafe { test(&raw const *name) }
}

/// example.com for www.example.com and accounts.example.com; bbc.co.uk stays bbc.co.uk
/// (Search's `Registrable.domain`).
pub fn registrable(host: &str) -> String {
    let test: &dyn Fn(&str) -> bool = &suffix;
    domain_of(host, is_suffix().map(|_| test))
}

fn domain_of(host: &str, is_suffix: Option<&dyn Fn(&str) -> bool>) -> String {
    let name = host.to_lowercase();
    let name = name.trim_matches('.');
    let labels: Vec<&str> = name.split('.').collect();
    let address = name.contains(':')
        || labels
            .iter()
            .all(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_digit()));
    let Some(is_suffix) = is_suffix else {
        return name.to_owned();
    };
    if address || labels.len() < 2 || is_suffix(name) {
        return name.to_owned();
    }
    for count in (1..labels.len()).rev() {
        if is_suffix(&labels[labels.len() - count..].join(".")) {
            return labels[labels.len() - count - 1..].join(".");
        }
    }
    name.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(name: &str) -> bool {
        ["com", "uk", "co.uk", "io", "github.io"].contains(&name)
    }

    #[test]
    fn a_site_is_its_registrable_domain() {
        let test: &dyn Fn(&str) -> bool = &known;
        assert_eq!(domain_of("accounts.example.com", Some(test)), "example.com");
        assert_eq!(domain_of("www.bbc.co.uk", Some(test)), "bbc.co.uk");
        assert_eq!(domain_of("bbc.co.uk", Some(test)), "bbc.co.uk");
        // A suffix anyone can have a name under: each name its own site.
        assert_eq!(domain_of("me.github.io", Some(test)), "me.github.io");
        assert_eq!(domain_of("github.io", Some(test)), "github.io");
        assert_eq!(domain_of("192.168.1.1", Some(test)), "192.168.1.1");
        // No list: a host is only its own site.
        assert_eq!(
            domain_of("accounts.example.com", None),
            "accounts.example.com"
        );
    }

    #[test]
    fn the_system_knows_its_suffixes() {
        assert_eq!(registrable("accounts.google.com"), "google.com");
        assert_eq!(registrable("www.bbc.co.uk"), "bbc.co.uk");
    }
}
