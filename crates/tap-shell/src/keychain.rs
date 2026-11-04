//! The macOS keychain, as Search's `Vault.swift` and `Import.swift` ask it: internet
//! passwords under a label (Tap's own, or Search's to read), and a browser's "Safe
//! Storage" key, a generic password. Search's `SecItem` dictionaries, one to one, built
//! as Foundation dictionaries (toll-free bridged to the `CFDictionary` the calls take).
//!
//! Nothing here logs a secret. A list never holds one: the keychain hands over either
//! many items' attributes or one item's secret, not both in one call (asked for every
//! item's data at once it answers `errSecParam`, Search found), so secrets are read one
//! at a time, and only when one is wanted.

use objc2::{rc::Retained, runtime::AnyObject};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSNumber, NSString};

type OsStatus = i32;
const SUCCESS: OsStatus = 0;
const NOT_FOUND: OsStatus = -25300;

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    static kSecClass: &'static NSString;
    static kSecClassInternetPassword: &'static NSString;
    static kSecClassGenericPassword: &'static NSString;
    static kSecAttrLabel: &'static NSString;
    static kSecAttrServer: &'static NSString;
    static kSecAttrAccount: &'static NSString;
    static kSecAttrService: &'static NSString;
    static kSecAttrComment: &'static NSString;
    static kSecAttrSecurityDomain: &'static NSString;
    static kSecAttrProtocol: &'static NSString;
    static kSecAttrProtocolHTTP: &'static NSString;
    static kSecAttrProtocolHTTPS: &'static NSString;
    static kSecAttrAuthenticationType: &'static NSString;
    static kSecAttrAuthenticationTypeHTMLForm: &'static NSString;
    static kSecAttrAccessible: &'static NSString;
    static kSecAttrAccessibleWhenUnlocked: &'static NSString;
    static kSecReturnAttributes: &'static NSString;
    static kSecReturnData: &'static NSString;
    static kSecMatchLimit: &'static NSString;
    static kSecMatchLimitAll: &'static NSString;
    static kSecMatchLimitOne: &'static NSString;
    static kSecValueData: &'static NSString;

    fn SecItemCopyMatching(query: *const AnyObject, result: *mut *mut AnyObject) -> OsStatus;
    fn SecItemAdd(attributes: *const AnyObject, result: *mut *mut AnyObject) -> OsStatus;
    fn SecItemUpdate(query: *const AnyObject, attributes: *const AnyObject) -> OsStatus;
    fn SecItemDelete(query: *const AnyObject) -> OsStatus;
}

/// One item as a list needs it: no secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub server: String,
    /// "" for an item kept under no name: the keychain lists such an item with no
    /// account at all, not an empty one (Search's `account(of:)`).
    pub account: String,
    /// The keychain has no "last used"; Search keeps it in the comment, as seconds.
    pub comment: Option<String>,
    /// Kept from a page sent over plain http.
    pub clear: bool,
}

/// Whose items: a label, and for Tap's own a security domain. The keychain knows an
/// internet password by its server, account, protocol, kind, port, path and security
/// domain — never by its label — so without a domain of its own, Tap's copy of an
/// account Search keeps would be refused as a duplicate of Search's.
#[derive(Clone, Copy)]
pub struct Drawer<'a> {
    pub label: &'a str,
    pub domain: Option<&'a str>,
}

type Dict = Retained<NSDictionary<NSString, AnyObject>>;

fn dict(pairs: &[(&NSString, &AnyObject)]) -> Dict {
    let keys: Vec<&NSString> = pairs.iter().map(|p| p.0).collect();
    let values: Vec<&AnyObject> = pairs.iter().map(|p| p.1).collect();
    NSDictionary::from_slices(&keys, &values)
}

fn s(text: &str) -> Retained<NSString> {
    NSString::from_str(text)
}

fn yes() -> Retained<NSNumber> {
    NSNumber::new_bool(true)
}

fn data(text: &str) -> Retained<NSData> {
    NSData::with_bytes(text.as_bytes())
}

fn text_of(dict: &NSDictionary<NSString, AnyObject>, key: &NSString) -> Option<String> {
    let value = dict.objectForKey(key)?;
    value.downcast::<NSString>().ok().map(|s| s.to_string())
}

/// Calls `SecItemCopyMatching` and takes ownership of what it answers.
fn copy_matching(query: &Dict) -> Result<Retained<AnyObject>, OsStatus> {
    let mut out: *mut AnyObject = std::ptr::null_mut();
    // SAFETY: a valid dictionary, and an out-pointer the call fills with a +1 object.
    let status = unsafe { SecItemCopyMatching(Retained::as_ptr(query).cast(), &raw mut out) };
    if status != SUCCESS {
        return Err(status);
    }
    // SAFETY: on success the result is a retained Core Foundation object we now own.
    unsafe { Retained::from_raw(out) }.ok_or(NOT_FOUND)
}

/// Every internet password under `label`, narrowed to `server` if given: attributes
/// only, never a secret. Reading attributes asks nothing of anyone, whichever app's
/// items they are.
pub fn rows(label: &str, server: Option<&str>) -> Vec<Row> {
    // SAFETY: the Security framework's constant keys, valid for the process's life.
    let found = unsafe {
        let label = s(label);
        let server = server.map(s);
        let yes = yes();
        let mut pairs: Vec<(&NSString, &AnyObject)> = vec![
            (kSecClass, kSecClassInternetPassword.as_ref()),
            (kSecAttrLabel, label.as_ref()),
            (kSecReturnAttributes, yes.as_ref()),
            (kSecMatchLimit, kSecMatchLimitAll.as_ref()),
        ];
        if let Some(server) = &server {
            pairs.push((kSecAttrServer, server.as_ref()));
        }
        copy_matching(&dict(&pairs))
    };
    let Ok(found) = found else {
        return Vec::new();
    };
    let Ok(list) = found.downcast::<NSArray>() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in &list {
        let Ok(row) = item.downcast::<NSDictionary>() else {
            continue;
        };
        // SAFETY: an attributes dictionary is keyed by strings.
        let row: Retained<NSDictionary<NSString, AnyObject>> =
            unsafe { Retained::cast_unchecked(row) };
        // SAFETY: the framework's constant keys.
        let (server, account, comment, protocol, http) = unsafe {
            (
                text_of(&row, kSecAttrServer),
                text_of(&row, kSecAttrAccount),
                text_of(&row, kSecAttrComment),
                text_of(&row, kSecAttrProtocol),
                kSecAttrProtocolHTTP.to_string(),
            )
        };
        let Some(server) = server else { continue };
        out.push(Row {
            server,
            account: account.unwrap_or_default(),
            comment,
            clear: protocol.as_deref() == Some(http.as_str()),
        });
    }
    out
}

/// One item's secret, by the two things that name it. An item another app made (Search's)
/// is the keychain's to hand over: macOS asks first, and this waits for the answer, so
/// it is never called on the main thread for one of those.
pub fn secret(drawer: Drawer, server: &str, account: &str) -> Option<String> {
    let (label, server, account, yes) = (s(drawer.label), s(server), s(account), yes());
    let domain = drawer.domain.map(s);
    // SAFETY: the framework's constant keys and values.
    let found = unsafe {
        let mut query: Vec<(&NSString, &AnyObject)> = vec![
            (kSecClass, kSecClassInternetPassword.as_ref()),
            (kSecAttrLabel, label.as_ref()),
            (kSecAttrServer, server.as_ref()),
            (kSecAttrAccount, account.as_ref()),
            (kSecReturnData, yes.as_ref()),
            (kSecMatchLimit, kSecMatchLimitOne.as_ref()),
        ];
        if let Some(domain) = &domain {
            query.push((kSecAttrSecurityDomain, domain.as_ref()));
        }
        copy_matching(&dict(&query))
    };
    let data = found.ok()?.downcast::<NSData>().ok()?;
    String::from_utf8(data.to_vec()).ok()
}

/// Keeps a password under `label`: updated if this label already keeps one for the
/// server and account, added if not. Ours only: server and account alone also match
/// what other apps keep (git's token for github.com under your name), and an update
/// would write over it.
pub fn save(
    drawer: Drawer,
    server: &str,
    account: &str,
    password: &str,
    used: Option<f64>,
    clear: bool,
) -> bool {
    if server.is_empty() || password.is_empty() {
        return false;
    }
    let (label, server, account) = (s(drawer.label), s(server), s(account));
    let domain = drawer.domain.map(s);
    let secret = data(password);
    let used = used.map(|u| s(&u.to_string()));
    // SAFETY: the framework's constant keys and values, and valid dictionaries.
    unsafe {
        let mut identity: Vec<(&NSString, &AnyObject)> = vec![
            (kSecClass, kSecClassInternetPassword.as_ref()),
            (kSecAttrServer, server.as_ref()),
            (kSecAttrAccount, account.as_ref()),
            (kSecAttrLabel, label.as_ref()),
        ];
        if let Some(domain) = &domain {
            identity.push((kSecAttrSecurityDomain, domain.as_ref()));
        }
        let identity = dict(&identity);
        // A web form's, which also keeps ours apart from another app's item for the
        // same server, account and protocol (git's), which would make adding fail.
        let protocol: &NSString = if clear {
            kSecAttrProtocolHTTP
        } else {
            kSecAttrProtocolHTTPS
        };
        let mut fields: Vec<(&NSString, &AnyObject)> = vec![
            (kSecValueData, secret.as_ref()),
            (kSecAttrLabel, label.as_ref()),
            (
                kSecAttrAuthenticationType,
                kSecAttrAuthenticationTypeHTMLForm.as_ref(),
            ),
            (kSecAttrProtocol, protocol.as_ref()),
        ];
        if let Some(used) = &used {
            fields.push((kSecAttrComment, used.as_ref()));
        }
        let status = SecItemUpdate(
            Retained::as_ptr(&identity).cast(),
            Retained::as_ptr(&dict(&fields)).cast(),
        );
        if status == SUCCESS {
            return true;
        }
        if status != NOT_FOUND {
            return false;
        }
        let mut fresh: Vec<(&NSString, &AnyObject)> = vec![
            (kSecClass, kSecClassInternetPassword.as_ref()),
            (kSecAttrServer, server.as_ref()),
            (kSecAttrAccount, account.as_ref()),
            (kSecAttrAccessible, kSecAttrAccessibleWhenUnlocked.as_ref()),
        ];
        if let Some(domain) = &domain {
            fresh.push((kSecAttrSecurityDomain, domain.as_ref()));
        }
        fresh.extend(fields);
        SecItemAdd(Retained::as_ptr(&dict(&fresh)).cast(), std::ptr::null_mut()) == SUCCESS
    }
}

/// Takes one of `label`'s items out. Whether the keychain let it go.
pub fn forget(drawer: Drawer, server: &str, account: &str) -> bool {
    let (label, server, account) = (s(drawer.label), s(server), s(account));
    let domain = drawer.domain.map(s);
    // SAFETY: the framework's constant keys and a valid dictionary.
    unsafe {
        let mut query: Vec<(&NSString, &AnyObject)> = vec![
            (kSecClass, kSecClassInternetPassword.as_ref()),
            (kSecAttrServer, server.as_ref()),
            (kSecAttrAccount, account.as_ref()),
            (kSecAttrLabel, label.as_ref()),
        ];
        if let Some(domain) = &domain {
            query.push((kSecAttrSecurityDomain, domain.as_ref()));
        }
        SecItemDelete(Retained::as_ptr(&dict(&query)).cast()) == SUCCESS
    }
}

/// A browser's "Safe Storage" key: a generic password it keeps for itself. macOS asks
/// you before handing it to Tap — the one question a Chromium browser's passwords
/// take — and this waits for the answer, so it runs off the main thread.
pub fn generic(service: &str, account: &str) -> Option<String> {
    let (service, account, yes) = (s(service), s(account), yes());
    // SAFETY: the framework's constant keys and a valid dictionary.
    let found = unsafe {
        copy_matching(&dict(&[
            (kSecClass, kSecClassGenericPassword.as_ref()),
            (kSecAttrService, service.as_ref()),
            (kSecAttrAccount, account.as_ref()),
            (kSecReturnData, yes.as_ref()),
            (kSecMatchLimit, kSecMatchLimitOne.as_ref()),
        ]))
    };
    let data = found.ok()?.downcast::<NSData>().ok()?;
    String::from_utf8(data.to_vec())
        .ok()
        .filter(|t| !t.is_empty())
}
