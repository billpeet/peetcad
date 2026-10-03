//! The open documents: what the application and a command line run work on.
//!
//! A [`Session`] holds one or more [`Document`]s, one of them *current*. It dereferences
//! to the current document, so code that works on "the document" works on a session
//! unchanged; what a session adds is opening a document beside the others, switching
//! between them and closing one. A session is never empty: closing the last document
//! leaves a new, empty one.

use std::ops::{Deref, DerefMut};

use crate::document::Document;

/// Identifies a document within its session. Ids are not reused, so one that was
/// closed is never mistaken for another.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocId(pub u32);

pub struct Session {
    /// In the order they were opened.
    docs: Vec<(DocId, Document)>,
    /// Index into `docs`.
    current: usize,
    next: u32,
}

impl Default for Session {
    fn default() -> Self {
        Self::new(Document::default())
    }
}

impl From<Document> for Session {
    fn from(doc: Document) -> Self {
        Self::new(doc)
    }
}

impl Deref for Session {
    type Target = Document;

    fn deref(&self) -> &Document {
        &self.docs[self.current].1
    }
}

impl DerefMut for Session {
    fn deref_mut(&mut self) -> &mut Document {
        &mut self.docs[self.current].1
    }
}

impl Session {
    /// A session with one document.
    pub fn new(doc: Document) -> Self {
        Self {
            docs: vec![(DocId(1), doc)],
            current: 0,
            next: 2,
        }
    }

    /// The id of the current document.
    pub fn current_id(&self) -> DocId {
        self.docs[self.current].0
    }

    /// How many documents are open (at least one).
    pub fn count(&self) -> usize {
        self.docs.len()
    }

    /// The open documents, in the order they were opened.
    pub fn documents(&self) -> impl ExactSizeIterator<Item = (DocId, &Document)> {
        self.docs.iter().map(|(id, doc)| (*id, doc))
    }

    pub fn get(&self, id: DocId) -> Option<&Document> {
        self.docs.iter().find(|(i, _)| *i == id).map(|(_, d)| d)
    }

    pub fn get_mut(&mut self, id: DocId) -> Option<&mut Document> {
        self.docs.iter_mut().find(|(i, _)| *i == id).map(|(_, d)| d)
    }

    /// Opens `doc` beside the others and makes it current. Returns its id.
    pub fn open(&mut self, doc: Document) -> DocId {
        let id = DocId(self.next);
        self.next += 1;
        self.docs.push((id, doc));
        self.current = self.docs.len() - 1;
        id
    }

    /// Makes the document current. Returns whether there is such a document.
    pub fn switch(&mut self, id: DocId) -> bool {
        match self.docs.iter().position(|(i, _)| *i == id) {
            Some(index) => {
                self.current = index;
                true
            }
            None => false,
        }
    }

    /// Closes the document and returns it. If it was current, the one opened before it
    /// becomes current (the next one if it was the first); if it was the only one, a
    /// new, empty document takes its place.
    pub fn close(&mut self, id: DocId) -> Option<Document> {
        let index = self.docs.iter().position(|(i, _)| *i == id)?;
        let (_, doc) = self.docs.remove(index);
        if self.docs.is_empty() {
            let id = DocId(self.next);
            self.next += 1;
            self.docs.push((id, Document::default()));
            self.current = 0;
        } else if index < self.current || (index == self.current && index > 0) {
            self.current -= 1;
        }
        Some(doc)
    }

    /// Whether any open document has unsaved changes.
    pub fn any_modified(&self) -> bool {
        self.docs.iter().any(|(_, d)| d.is_modified())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str) -> Document {
        let mut model = peet_model::Model::new();
        name.clone_into(&mut model.name);
        Document::from_model(model, None)
    }

    fn names(s: &Session) -> Vec<String> {
        s.documents().map(|(_, d)| d.title()).collect()
    }

    #[test]
    fn documents_are_opened_switched_and_closed() {
        let mut s = Session::new(named("A"));
        let a = s.current_id();
        assert_eq!((s.count(), s.title().as_str()), (1, "A"));
        let b = s.open(named("B"));
        let c = s.open(named("C"));
        assert_eq!(s.current_id(), c);
        assert_eq!(names(&s), ["A", "B", "C"]);

        // The session is its current document.
        assert!(s.change("Rename", |m| m.name = "C2".to_owned()));
        assert_eq!(s.get(c).unwrap().title(), "C2");
        assert!(s.any_modified() && !s.get(a).unwrap().is_modified());

        // Closing another document keeps the current one current.
        assert!(s.switch(b));
        assert_eq!(s.close(a).unwrap().title(), "A");
        assert_eq!(s.current_id(), b);
        assert!(s.get(a).is_none() && !s.switch(a) && s.close(a).is_none());
        // Closing the current one goes to the one opened before it, or the next.
        assert!(s.switch(c));
        s.close(c);
        assert_eq!(s.current_id(), b);
        let d = s.open(named("D"));
        assert!(s.switch(b));
        s.close(b);
        assert_eq!(s.current_id(), d);

        // The last one leaves an empty document, under a new id.
        s.close(d);
        assert_eq!(s.count(), 1);
        assert!(s.current_id() > d);
        assert!(s.model.is_empty() && !s.any_modified());
    }
}
