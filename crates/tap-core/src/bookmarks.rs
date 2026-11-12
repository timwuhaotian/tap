//! The places you kept, as `bookmarks.json` keeps them (Search's `Bookmarks.swift`):
//! a list of bookmarks, each one either a site with an address of its own or a folder
//! holding others.
//!
//! The file is the one the importers write, so a place kept by hand and one brought
//! over from Chrome live in one list, and a folder that came from elsewhere is never
//! lost on the way out.

use serde::{Deserialize, Serialize};

use crate::fnd;

/// One bookmark, as the file keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bookmark {
    /// Its own identity, as Search writes it: an upper-case UUID.
    pub id: String,
    /// The name it is listed under.
    pub title: String,
    /// Where it goes. Absent for a folder.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// What it holds. Absent for a site.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<Bookmark>>,
}

impl Bookmark {
    /// A site: a name and an address.
    #[must_use]
    pub fn site(title: &str, url: &str) -> Bookmark {
        Bookmark {
            id: fnd::uuid(),
            title: title.to_owned(),
            url: Some(url.to_owned()),
            children: None,
        }
    }

    /// Whether it holds others rather than going somewhere itself.
    #[must_use]
    pub fn is_folder(&self) -> bool {
        self.url.is_none()
    }
}

/// One row of the list of what is kept: a folder's name, or a place to go to.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    /// A folder: its name, and how deep in the list it sits.
    Folder { depth: usize, title: String },
    /// A place: its name, its address, and how deep in the list it sits.
    Site {
        depth: usize,
        title: String,
        url: String,
    },
}

/// The places you kept.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bookmarks {
    roots: Vec<Bookmark>,
}

impl Bookmarks {
    /// The list as the file keeps it; `None` when the file is not one of ours.
    #[must_use]
    pub fn load(data: &[u8]) -> Option<Self> {
        let roots: Vec<Bookmark> = serde_json::from_slice(data).ok()?;
        Some(Self { roots })
    }

    /// The file's own bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(&self.roots).unwrap_or_default()
    }

    /// The bookmarks, as they are kept.
    #[must_use]
    pub fn roots(&self) -> &[Bookmark] {
        &self.roots
    }

    /// Whether a place is already kept, wherever it is kept (Search's `contains`).
    #[must_use]
    pub fn contains(&self, url: &str) -> bool {
        self.roots.iter().any(|node| holds(node, url))
    }

    /// Keeps a place, at the end of the list. Nothing is asked first: the name is the
    /// page's own (Search's `add`). Answers whether it was kept.
    pub fn add(&mut self, url: &str, title: &str) -> bool {
        if url.is_empty() || self.contains(url) {
            return false;
        }
        self.roots.push(Bookmark::site(title, url));
        true
    }

    /// Lets a place go, wherever in the list it is kept. Answers whether it went.
    pub fn remove(&mut self, url: &str) -> bool {
        let mut taken = false;
        self.roots.retain_mut(|node| {
            if node.url.as_deref() == Some(url) {
                taken = true;
                return false;
            }
            // A folder that holds it keeps its own place in the list.
            if release(node, url) {
                taken = true;
            }
            true
        });
        taken
    }

    /// Every row of the list of what is kept: the folders and the places under them,
    /// in the order the file keeps them.
    #[must_use]
    pub fn entries(&self) -> Vec<Entry> {
        let mut rows = Vec::new();
        for node in &self.roots {
            walk(node, 0, &mut rows);
        }
        rows
    }

    /// How many places are kept, folders aside (the importers' `bookmark_count`).
    #[must_use]
    pub fn count(&self) -> usize {
        let mut count = 0;
        for node in &self.roots {
            count += sites(node);
        }
        count
    }
}

/// One node and everything under it.
fn walk(node: &Bookmark, depth: usize, rows: &mut Vec<Entry>) {
    if let Some(url) = &node.url {
        rows.push(Entry::Site {
            depth,
            title: node.title.clone(),
            url: url.clone(),
        });
        return;
    }
    let title = node.title.clone();
    rows.push(Entry::Folder { depth, title });
    if let Some(kids) = &node.children {
        for kid in kids {
            walk(kid, depth + 1, rows);
        }
    }
}

/// How many places a node holds, itself aside.
fn sites(node: &Bookmark) -> usize {
    match (&node.url, &node.children) {
        (Some(_), _) => 1,
        (None, Some(kids)) => kids.iter().map(sites).sum(),
        (None, None) => 0,
    }
}

/// Whether a node, or anything under it, is the place.
fn holds(node: &Bookmark, url: &str) -> bool {
    node.url.as_deref() == Some(url)
        || node
            .children
            .as_ref()
            .is_some_and(|kids| kids.iter().any(|kid| holds(kid, url)))
}

/// Takes the place out of a node's children, and out of any folder under it. The node
/// itself stays: it is a folder, and it still holds what it held.
fn release(node: &mut Bookmark, url: &str) -> bool {
    let Some(kids) = node.children.as_mut() else {
        return false;
    };
    let mut taken = false;
    kids.retain_mut(|kid| {
        if kid.url.as_deref() == Some(url) {
            taken = true;
            false
        } else {
            if release(kid, url) {
                taken = true;
            }
            true
        }
    });
    taken
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_place_is_kept_once_and_let_go_anywhere() {
        let mut marks = Bookmarks::default();
        assert!(marks.add("https://example.com", "Example"));
        assert!(!marks.add("https://example.com", "Example again"));
        assert!(marks.contains("https://example.com"));
        assert_eq!(marks.count(), 1);
        assert!(marks.remove("https://example.com"));
        assert!(!marks.contains("https://example.com"));
        assert!(!marks.remove("https://example.com"));
    }

    #[test]
    fn a_folder_from_elsewhere_is_kept_and_its_places_are_counted() {
        // What an importer writes: a folder with places under it.
        let file = br#"[{"id":"A","title":"Reading","children":[
            {"id":"B","title":"Docs","url":"https://example.com/docs"}]}]"#;
        let mut marks = Bookmarks::load(file).expect("our own file");
        assert_eq!(marks.count(), 1);
        assert!(marks.contains("https://example.com/docs"));
        let rows = marks.entries();
        assert_eq!(rows.len(), 2);
        assert!(matches!(&rows[0], Entry::Folder { depth: 0, title } if title == "Reading"));
        assert!(
            matches!(&rows[1], Entry::Site { depth: 1, url, .. } if url == "https://example.com/docs")
        );
        // A place inside the folder goes, and the folder stays.
        assert!(marks.remove("https://example.com/docs"));
        assert_eq!(marks.count(), 0);
        assert_eq!(marks.roots().len(), 1);
        assert!(marks.roots()[0].is_folder());
    }

    #[test]
    fn what_is_written_is_what_was_read() {
        let mut marks = Bookmarks::default();
        marks.add("https://example.com", "Example");
        let file = marks.encode();
        let again = Bookmarks::load(&file).expect("our own file");
        assert_eq!(again, marks);
        // A site has no children and a folder has no address, as Search writes them.
        let text = String::from_utf8(file).expect("text");
        assert!(!text.contains("children"));
        assert!(!text.contains("\"url\":null"));
    }
}
