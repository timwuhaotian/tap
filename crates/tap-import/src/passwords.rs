//! Passwords as they come in from elsewhere (Search's `Vault.host(of:)` and
//! `Vault.take(csv:)`): the site a login belongs to, and a CSV as Google Password Manager,
//! Chrome or Dia export it. Nothing here touches the keychain or keeps a copy: the
//! keychain is the shell's, and a file read here is the person's to delete.

/// One login brought in: the site, the account, the password, and when it was last used
/// to sign in (seconds since 1970), if the other browser kept that.
#[derive(Clone, Debug, PartialEq)]
pub struct Login {
    pub host: String,
    pub user: String,
    pub password: String,
    pub used: Option<f64>,
    /// Kept from a page sent in the clear, over plain http. Only such logins are offered
    /// on such a page (Search's `Login.clear`).
    pub clear: bool,
}

impl Login {
    /// What names it among the rest: its site and its account, as the keychain does.
    #[must_use]
    pub fn id(&self) -> String {
        format!("{}\u{1}{}", self.host, self.user)
    }
}

/// The site a login is for: `example.com` for `https://www.example.com/login`, lowercased,
/// `www.` dropped, and a bare `example.com` taken as https. A website's, and nothing else:
/// an Android app's login in an export (`android://…@com.vendor.app/`) names a package,
/// which can read as a domain somebody else owns, and would be offered to them. Anything
/// that isn't an http or https address is "".
#[must_use]
pub fn host_of(text: &str) -> String {
    let text = text.trim_matches(|c: char| c == ' ' || c == '\t' || c == '\u{a0}');
    let (scheme, rest) = match text.split_once("://") {
        Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest),
        None => ("https".to_owned(), text),
    };
    if scheme != "http" && scheme != "https" {
        return String::new();
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    // Who signs in, before an @, isn't the site.
    let hostport = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if let Some(inside) = hostport.strip_prefix('[') {
        // An IPv6 address, without its brackets, as Foundation gives it.
        match inside.split_once(']') {
            Some((address, _)) => address,
            None => return String::new(),
        }
    } else {
        hostport.split(':').next().unwrap_or_default()
    };
    // What a URL can't hold: an address that doesn't parse names no site.
    if host.is_empty()
        || host
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "<>\"{}|\\^`".contains(c))
    {
        return String::new();
    }
    let host = host.to_lowercase();
    match host.strip_prefix("www.") {
        Some(bare) if !bare.is_empty() => bare.to_owned(),
        _ => host,
    }
}

/// What an exported CSV holds: the logins worth keeping, and how many rows weren't (no
/// site, no password, an app's, a row too short).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Csv {
    pub logins: Vec<Login>,
    pub skipped: usize,
}

/// A CSV as Google Password Manager, Chrome or Dia write it: name, url, username,
/// password, note — found by the header's names, in any order, under the names other
/// managers give them too. A file without a site, an account or a password column keeps
/// nothing, and every row in it counts as skipped.
#[must_use]
pub fn csv(text: &str) -> Csv {
    let mut rows = parse(text).into_iter();
    let Some(header) = rows.next() else {
        return Csv::default();
    };
    let header: Vec<String> = header.iter().map(|h| h.to_lowercase()).collect();
    let column = |names: &[&str]| header.iter().position(|h| names.contains(&h.as_str()));
    let rows: Vec<Vec<String>> = rows.collect();
    let (Some(url_at), Some(user_at), Some(pass_at)) = (
        column(&["url", "login_uri", "website", "site"]),
        column(&["username", "login_username", "user", "email"]),
        column(&["password", "login_password"]),
    ) else {
        return Csv {
            logins: Vec::new(),
            skipped: rows.len(),
        };
    };
    let mut out = Csv::default();
    let widest = url_at.max(user_at).max(pass_at);
    for row in rows {
        if row.len() <= widest {
            out.skipped += 1;
            continue;
        }
        let host = host_of(&row[url_at]);
        let password = &row[pass_at];
        if host.is_empty() || password.is_empty() {
            out.skipped += 1;
            continue;
        }
        let clear = row[url_at]
            .trim_matches(|c: char| c == ' ' || c == '\t')
            .to_lowercase()
            .starts_with("http://");
        out.logins.push(Login {
            host,
            user: row[user_at].clone(),
            password: password.clone(),
            used: None,
            clear,
        });
    }
    out
}

/// The field read last ends its row; a row with nothing in it is no row.
fn end_row(row: &mut Vec<String>, field: &mut String, rows: &mut Vec<Vec<String>>) {
    row.push(std::mem::take(field));
    if row.iter().any(|f| !f.is_empty()) {
        rows.push(std::mem::take(row));
    } else {
        row.clear();
    }
}

/// Quoted fields, doubled quotes inside them, and newlines inside those — all three turn
/// up in a real export. A line with nothing in it is no row.
fn parse(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            ',' => row.push(std::mem::take(&mut field)),
            '\r' => {
                // \r\n is one line's end, as a lone \r is.
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                end_row(&mut row, &mut field, &mut rows);
            }
            '\n' => end_row(&mut row, &mut field, &mut rows),
            _ => field.push(c),
        }
    }
    end_row(&mut row, &mut field, &mut rows);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_is_the_site_without_www() {
        assert_eq!(
            host_of("https://www.example.com/login?next=/"),
            "example.com"
        );
        assert_eq!(host_of("  Example.COM  "), "example.com");
        assert_eq!(
            host_of("http://accounts.example.com"),
            "accounts.example.com"
        );
        assert_eq!(host_of("https://me@Example.com:8443/x"), "example.com");
        assert_eq!(host_of("https://[::1]:8080/"), "::1");
    }

    #[test]
    fn the_host_is_empty_for_anything_but_a_website() {
        assert_eq!(host_of("android://hash@com.vendor.app/"), "");
        assert_eq!(host_of("ftp://example.com"), "");
        assert_eq!(host_of(""), "");
        assert_eq!(host_of("https://"), "");
        assert_eq!(host_of("https://exa mple.com/"), "");
    }

    #[test]
    fn an_export_without_a_password_column_is_skipped_whole() {
        let read = csv("url,username\nhttps://example.com,me\nhttps://example.org,you\n");
        assert_eq!(read.logins, Vec::<Login>::new());
        assert_eq!(read.skipped, 2);
    }

    #[test]
    fn an_empty_export_keeps_and_skips_nothing() {
        assert_eq!(csv(""), Csv::default());
    }

    #[test]
    fn rows_that_cannot_be_kept_are_counted_as_skipped() {
        let read = csv("name,url,username,password,note\n\
             no password,https://example.com,me,,\n\
             no site,,me,secret,\n\
             an app,android://hash@com.vendor.app/,me,secret,\n\
             too short,https://example.com");
        assert_eq!(read.logins, Vec::<Login>::new());
        assert_eq!(read.skipped, 4);
    }

    #[test]
    fn quoted_fields_with_newlines_and_quotes_stay_one_row() {
        // If the newline inside the quotes split the row, this would count two skipped.
        let read = csv(
            "url,username,password\nandroid://hash@com.vendor.app/,\"a \"\"quoted\"\"\nname\",secret\n",
        );
        assert_eq!(read.logins, Vec::<Login>::new());
        assert_eq!(read.skipped, 1);
        let kept = csv(
            "url,username,password\nhttps://example.com,\"a \"\"quoted\"\"\nname\",\"se,cret\"\n",
        );
        assert_eq!(kept.logins[0].user, "a \"quoted\"\nname");
        assert_eq!(kept.logins[0].password, "se,cret");
    }

    #[test]
    fn windows_line_endings_and_blank_lines_do_not_add_rows() {
        let read = csv("url,username,password\r\nandroid://x@com.app/,me,secret\r\n\r\n");
        assert_eq!(read.skipped, 1);
        assert_eq!(read.logins, Vec::<Login>::new());
    }

    #[test]
    fn the_columns_are_found_by_their_names_under_other_managers_names_too() {
        let read = csv("login_password,Name,LOGIN_URI,login_username\r\n\
             p1,Example,https://www.example.com/,alice\r\n\
             p2,Plain,http://intranet.example/,bob\r\n\
             p3,Bare,example.org,\r\n");
        assert_eq!(read.skipped, 0);
        let got: Vec<(&str, &str, &str, bool)> = read
            .logins
            .iter()
            .map(|l| {
                (
                    l.host.as_str(),
                    l.user.as_str(),
                    l.password.as_str(),
                    l.clear,
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("example.com", "alice", "p1", false),
                ("intranet.example", "bob", "p2", true),
                ("example.org", "", "p3", false),
            ]
        );
        assert_eq!(read.logins[0].id(), "example.com\u{1}alice");
    }
}
