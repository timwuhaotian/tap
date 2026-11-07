//! Where a download goes and what it is called (Search's `Browser.fileName`,
//! `Browser.free` and `Prefs.downloads`).

use std::path::{Path, PathBuf};

/// The name a download is saved under: the last part of what the server or the page
/// suggested, never a path, and never nothing.
#[must_use]
pub fn file_name(suggested: &str) -> String {
    let last = suggested.rsplit('/').next().unwrap_or("");
    if matches!(last, "" | "." | "..") {
        "download".to_owned()
    } else {
        last.to_owned()
    }
}

/// A free place for `name` in `folder`. WebKit refuses to write over a file that is
/// already there, so the name gains a number rather than the download quietly failing:
/// "report.pdf", then "report 2.pdf", "report 3.pdf".
#[must_use]
pub fn free(name: &str, folder: &Path) -> PathBuf {
    let candidate = folder.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, Some(extension)),
        _ => (name, None),
    };
    (2..=u32::MAX)
        .map(|n| match extension {
            Some(extension) => folder.join(format!("{stem} {n}.{extension}")),
            None => folder.join(format!("{stem} {n}")),
        })
        .find(|path| !path.exists())
        .unwrap_or(candidate)
}

/// How far the running downloads are, together, in whole percents (Search's
/// `Fetches.measure`): each entry's completed and total bytes. `None` while any of
/// them says no size, or while there is nothing to measure.
#[must_use]
pub fn aggregate(parts: &[(i64, i64)]) -> Option<f64> {
    let mut total = 0i64;
    let mut completed = 0i64;
    for (done, size) in parts {
        if *size <= 0 {
            return None;
        }
        total += size;
        completed += (*done).max(0);
    }
    #[allow(clippy::cast_precision_loss)] // downloads never reach 2^52 bytes
    (total > 0).then(|| (completed as f64 / total as f64 * 100.0).round() / 100.0)
}

/// Whether a response's `Content-Disposition` asks for the file to be kept, even one
/// WebKit could show: Gmail's download button loads the attachment into a hidden frame
/// and counts on exactly that.
#[must_use]
pub fn is_attachment(disposition: &str) -> bool {
    disposition.trim().to_lowercase().starts_with("attachment")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_the_last_part_and_never_nothing() {
        assert_eq!(file_name("report.pdf"), "report.pdf");
        assert_eq!(file_name("../../etc/passwd"), "passwd");
        assert_eq!(file_name("folder/"), "download");
        assert_eq!(file_name(".."), "download");
        assert_eq!(file_name(""), "download");
    }

    #[test]
    fn a_taken_name_gains_a_number() {
        let folder = std::env::temp_dir().join(format!("tap-fetching-{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("folder");
        assert_eq!(free("report.pdf", &folder), folder.join("report.pdf"));
        std::fs::write(folder.join("report.pdf"), "1").expect("file");
        assert_eq!(free("report.pdf", &folder), folder.join("report 2.pdf"));
        std::fs::write(folder.join("report 2.pdf"), "2").expect("file");
        assert_eq!(free("report.pdf", &folder), folder.join("report 3.pdf"));
        std::fs::write(folder.join("Makefile"), "m").expect("file");
        assert_eq!(free("Makefile", &folder), folder.join("Makefile 2"));
        std::fs::write(folder.join(".env"), "e").expect("file");
        assert_eq!(free(".env", &folder), folder.join(".env 2"));
        std::fs::remove_dir_all(&folder).expect("cleanup");
    }

    #[test]
    fn running_downloads_measure_together_in_whole_percents() {
        assert_eq!(aggregate(&[(50, 100), (25, 100)]), Some(0.38));
        assert_eq!(aggregate(&[(1, 3)]), Some(0.33));
        assert_eq!(aggregate(&[(100, 100)]), Some(1.0));
        assert_eq!(aggregate(&[]), None);
        assert_eq!(aggregate(&[(50, 100), (10, 0)]), None);
        assert_eq!(aggregate(&[(50, 100), (10, -1)]), None);
    }

    #[test]
    fn attachments_are_kept() {
        assert!(is_attachment(" Attachment; filename=\"a.pdf\""));
        assert!(!is_attachment("inline"));
    }
}
