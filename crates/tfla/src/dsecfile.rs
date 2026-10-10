use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::buff::{BufWriter, Buff};

// ── Magic ─────────────────────────────────────────────────────────────────────

const MAGIC: &[u8] = b"DSECFILE";

// ── Section ───────────────────────────────────────────────────────────────────

/// A named section of raw byte data inside a [`DSecFile`].
#[derive(Debug, Clone)]
pub struct Section {
    /// The section's unique name.
    pub name: String,
    /// Raw byte content of this section.
    pub data: Vec<u8>,
}

impl Section {
    /// Creates a new section with the given name and data.
    pub fn new(name: impl Into<String>, data: impl Into<Vec<u8>>) -> Self {
        Self {
            name: name.into(),
            data: data.into(),
        }
    }

    /// Returns the section's content as a byte slice.
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Attempts to interpret the section's content as a UTF-8 string.
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.data).ok()
    }

    /// Replaces this section's content entirely.
    pub fn set_data(&mut self, data: impl Into<Vec<u8>>) {
        self.data = data.into();
    }

    /// Appends bytes to this section's content.
    pub fn append(&mut self, bytes: impl AsRef<[u8]>) {
        self.data.extend_from_slice(bytes.as_ref());
    }

    /// Returns the number of bytes in this section.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Returns `true` if this section contains no bytes.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Converts this section's content into a `Buff` for easier parsing.
    pub fn as_buff(&self) -> Buff<'_> {
        Buff::new(self.as_bytes())
    }
}

// ── Error ─────────────────────────────────────────────────────────────────────

/// Errors that can occur when reading or writing a `.dsf` file.
#[derive(Debug)]
pub enum DSecError {
    /// The file does not start with the `DSECFILE` magic bytes.
    InvalidMagic,
    /// A section name or field in the file is malformed or not valid UTF-8.
    MalformedSection(String),
    /// A section with that name already exists.
    DuplicateName(String),
    /// No section with that name was found.
    NotFound(String),
    /// An underlying I/O error.
    Io(std::io::Error),
}

impl std::fmt::Display for DSecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidMagic => write!(f, "file does not begin with DSECFILE magic"),
            Self::MalformedSection(s) => write!(f, "malformed section: {s}"),
            Self::DuplicateName(n) => write!(f, "section already exists: '{n}'"),
            Self::NotFound(n) => write!(f, "section not found: '{n}'"),
            Self::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for DSecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        if let Self::Io(e) = self {
            Some(e)
        } else {
            None
        }
    }
}

impl From<std::io::Error> for DSecError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

type Result<T> = std::result::Result<T, DSecError>;

// ── DSecFile ──────────────────────────────────────────────────────────────────

/// A sectioned binary file manager for the `.dsf` format.
///
/// The on-disk format is:
/// ```text
/// DSECFILE [NAME: len-prefixed string] [SIZE: u32 LE] [content: SIZE bytes] …
/// ```
///
/// Sections are kept both in a `Vec` (to preserve insertion order) and in a
/// `HashMap` (for O(1) lookup by name).
///
/// # Examples
///
/// ```ignore
/// let mut file = DSecFile::new("save.dsf");
/// file.add("header", b"v1.0")?;
/// file.add("payload", b"\x00\x01\x02")?;
///
/// let header = file.get("header").unwrap();
/// assert_eq!(header.as_str(), Some("v1.0"));
///
/// file.save()?;
/// ```
pub struct DSecFile {
    path: PathBuf,
    /// Sections in insertion order.
    sections: Vec<Section>,
    /// Maps section name → index into `sections`.
    index: HashMap<String, usize>,
}

impl DSecFile {
    // ── Constructors ──────────────────────────────────────────────────────────

    /// Creates an empty `DSecFile` associated with the given path.
    ///
    /// Nothing is written to disk until [`save`](Self::save) is called.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            sections: Vec::new(),
            index: HashMap::new(),
        }
    }

    /// Reads and parses a `.dsf` file from disk.
    ///
    /// # Errors
    /// - [`DSecError::InvalidMagic`] if the magic header is missing.
    /// - [`DSecError::MalformedSection`] if a section is truncated or its name
    ///   is not valid UTF-8.
    /// - [`DSecError::Io`] on any filesystem error.
    pub fn from_file(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let bytes = std::fs::read(&path)?;
        Self::parse(&mut Buff::new(&bytes), path)
    }

    // ── Parsing ───────────────────────────────────────────────────────────────

    fn parse(buf: &mut Buff, path: PathBuf) -> Result<Self> {
        // Validate magic
        let magic = buf.next_bytes(MAGIC.len()).ok_or(DSecError::InvalidMagic)?;
        if magic != MAGIC {
            return Err(DSecError::InvalidMagic);
        }

        let mut file = Self::new(path);

        while !buf.is_exhausted() {
            let name = buf
                .next_string()
                .ok_or_else(|| DSecError::MalformedSection("could not read section name".into()))?;

            let size = buf.next_u32().ok_or_else(|| {
                DSecError::MalformedSection(format!("could not read size for section '{name}'"))
            })? as usize;

            let data = buf
                .next_bytes(size)
                .ok_or_else(|| {
                    DSecError::MalformedSection(format!(
                        "section '{name}' declares {size} bytes but the file is shorter"
                    ))
                })?
                .to_vec();

            file.add(name, data)?;
        }

        Ok(file)
    }

    // ── Accessors ─────────────────────────────────────────────────────────────

    /// Returns the file path associated with this `DSecFile`.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the number of sections.
    pub fn len(&self) -> usize {
        self.sections.len()
    }

    /// Returns `true` if there are no sections.
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    /// Returns `true` if a section with `name` exists.
    pub fn contains(&self, name: &str) -> bool {
        self.index.contains_key(name)
    }

    /// Returns an immutable reference to a section by name.
    ///
    /// Returns `None` if no section with that name exists.
    pub fn get(&self, name: &str) -> Option<&Section> {
        self.index.get(name).map(|&i| &self.sections[i])
    }

    /// Returns a mutable reference to a section by name.
    ///
    /// Returns `None` if no section with that name exists.
    pub fn get_mut(&mut self, name: &str) -> Option<&mut Section> {
        self.index.get(name).copied().map(|i| &mut self.sections[i])
    }

    /// Returns an iterator over all sections in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = &Section> {
        self.sections.iter()
    }

    /// Returns the names of all sections in insertion order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.sections.iter().map(|s| s.name.as_str())
    }

    // ── Mutation ──────────────────────────────────────────────────────────────

    /// Adds a new section. The name must be unique.
    ///
    /// # Errors
    /// Returns [`DSecError::DuplicateName`] if a section with this name already
    /// exists. Use [`set`](Self::set) to unconditionally overwrite.
    pub fn add(&mut self, name: impl Into<String>, data: impl Into<Vec<u8>>) -> Result<()> {
        let name = name.into();
        if self.index.contains_key(&name) {
            return Err(DSecError::DuplicateName(name));
        }
        let idx = self.sections.len();
        self.index.insert(name.clone(), idx);
        self.sections.push(Section::new(name, data));
        Ok(())
    }

    /// Overwrites an existing section's data, or adds it if it doesn't exist.
    pub fn set(&mut self, name: impl Into<String>, data: impl Into<Vec<u8>>) {
        let name = name.into();
        if let Some(&idx) = self.index.get(&name) {
            self.sections[idx].set_data(data);
        } else {
            let idx = self.sections.len();
            self.index.insert(name.clone(), idx);
            self.sections.push(Section::new(name, data));
        }
    }

    /// Removes a section by name, returning it if it existed.
    ///
    /// This is O(n) because it preserves section order and re-builds the index.
    pub fn remove(&mut self, name: &str) -> Option<Section> {
        let idx = self.index.remove(name)?;
        let removed = self.sections.remove(idx);

        // Shift every index that was above the removed element
        for i in idx..self.sections.len() {
            self.index.insert(self.sections[i].name.clone(), i);
        }

        Some(removed)
    }

    /// Renames a section, preserving its data and position.
    ///
    /// # Errors
    /// - [`DSecError::NotFound`] if `old_name` does not exist.
    /// - [`DSecError::DuplicateName`] if `new_name` is already taken.
    pub fn rename(&mut self, old_name: &str, new_name: impl Into<String>) -> Result<()> {
        let new_name = new_name.into();
        if self.index.contains_key(&new_name) {
            return Err(DSecError::DuplicateName(new_name));
        }
        let idx = self
            .index
            .remove(old_name)
            .ok_or_else(|| DSecError::NotFound(old_name.to_string()))?;
        self.sections[idx].name = new_name.clone();
        self.index.insert(new_name, idx);
        Ok(())
    }

    /// Appends bytes to an existing section.
    ///
    /// # Errors
    /// Returns [`DSecError::NotFound`] if no section with `name` exists.
    pub fn append(&mut self, name: &str, data: impl AsRef<[u8]>) -> Result<()> {
        let idx = *self
            .index
            .get(name)
            .ok_or_else(|| DSecError::NotFound(name.to_string()))?;
        self.sections[idx].append(data);
        Ok(())
    }

    // ── Serialisation ─────────────────────────────────────────────────────────

    /// Serialises all sections into a `BufWriter` (without writing to disk).
    ///
    /// Useful for inspection or embedding the format inside another structure.
    pub fn to_writer(&self) -> BufWriter {
        let mut buf = BufWriter::new();
        buf.write_bytes(MAGIC);
        for section in &self.sections {
            buf.write_string(&section.name);
            buf.write_u32(section.data.len() as u32);
            buf.write_bytes(&section.data);
        }
        buf
    }

    /// Writes the file to its associated path.
    ///
    /// # Errors
    /// Returns [`DSecError::Io`] on any filesystem error.
    pub fn save(&self) -> Result<()> {
        self.to_writer().save(&self.path).map_err(DSecError::Io)
    }

    /// Writes the file to a different path (does not change [`self.path`]).
    pub fn save_as(&self, path: impl AsRef<Path>) -> Result<()> {
        std::fs::write(path, self.to_writer().as_ref()).map_err(DSecError::Io)
    }
}

// ── Standard trait impls ──────────────────────────────────────────────────────

impl std::fmt::Debug for DSecFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DSecFile")
            .field("path", &self.path)
            .field(
                "sections",
                &self
                    .sections
                    .iter()
                    .map(|s| (&s.name, s.data.len()))
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// Allows iterating over sections with a `for` loop.
impl<'a> IntoIterator for &'a DSecFile {
    type Item = &'a Section;
    type IntoIter = std::slice::Iter<'a, Section>;

    fn into_iter(self) -> Self::IntoIter {
        self.sections.iter()
    }
}
