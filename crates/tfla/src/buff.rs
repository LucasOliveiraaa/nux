use std::path::Path;

// ── BufWriter (owned, write-side) ─────────────────────────────────────────────

/// An owned, append-only byte buffer for building binary data.
///
/// This is the write-side counterpart to [`Buff`]. Once you're done writing,
/// call [`freeze`](BufWriter::freeze) to get a read cursor over the accumulated
/// bytes, or [`into_bytes`](BufWriter::into_bytes) to take ownership of the
/// raw `Vec<u8>`.
///
/// # Examples
/// ```ignore
/// let mut w = BufWriter::new();
/// w.write_u32(42);
/// w.write_string("hello");
///
/// let mut r = w.freeze();
/// assert_eq!(r.next_u32(), Some(42));
/// assert_eq!(r.next_string().as_deref(), Some("hello"));
/// ```
#[derive(Debug, Default, Clone)]
pub struct BufWriter {
    data: Vec<u8>,
}

impl BufWriter {
    /// Creates an empty `BufWriter`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a `BufWriter` with a pre-allocated capacity (in bytes).
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            data: Vec::with_capacity(cap),
        }
    }

    // ── Sizing ────────────────────────────────────────────────────────────────

    /// Returns the number of bytes written so far.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Returns `true` if nothing has been written yet.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    // ── Writing ───────────────────────────────────────────────────────────────

    /// Appends raw bytes.
    pub fn write_bytes(&mut self, bytes: impl AsRef<[u8]>) {
        self.data.extend_from_slice(bytes.as_ref());
    }

    /// Appends a single byte.
    pub fn write_u8(&mut self, value: u8) {
        self.data.push(value);
    }

    /// Appends a little-endian `u16`.
    pub fn write_u16(&mut self, value: u16) {
        self.data.extend_from_slice(&value.to_le_bytes());
    }

    /// Appends a little-endian `u32`.
    pub fn write_u32(&mut self, value: u32) {
        self.data.extend_from_slice(&value.to_le_bytes());
    }

    /// Appends a little-endian `u64`.
    pub fn write_u64(&mut self, value: u64) {
        self.data.extend_from_slice(&value.to_le_bytes());
    }

    /// Appends a length-prefixed UTF-8 string (u32 byte-length then raw bytes).
    ///
    /// Pairs with [`Buff::next_string`].
    pub fn write_string(&mut self, value: &str) {
        self.write_u32(value.len() as u32);
        self.data.extend_from_slice(value.as_bytes());
    }

    // ── Conversion ────────────────────────────────────────────────────────────

    /// Returns a read cursor over the bytes written so far.
    ///
    /// The returned [`Buff`] borrows from `self`, so `self` must outlive the
    /// cursor. The cursor starts at position 0.
    pub fn freeze(&self) -> Buff<'_> {
        Buff::new(&self.data)
    }

    /// Consumes the writer and returns the underlying byte vector.
    pub fn into_bytes(self) -> Vec<u8> {
        self.data
    }

    /// Writes the accumulated bytes to `path`.
    pub fn save(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        std::fs::write(path, &self.data)
    }
}

impl AsRef<[u8]> for BufWriter {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}

impl From<Vec<u8>> for BufWriter {
    fn from(data: Vec<u8>) -> Self {
        Self { data }
    }
}

// ── Buff<'a> (borrowed, read-side) ────────────────────────────────────────────

/// A zero-copy read cursor over a borrowed byte slice.
///
/// `Buff<'a>` borrows its data from some external owner — a `Vec<u8>`, a
/// memory-mapped file, a `BufWriter`, a `&[u8]` literal, etc. It provides
/// sequential reads via `next_*` methods and arbitrary slicing, but never
/// copies or allocates.
///
/// # Lifetime
/// The `'a` lifetime ties the cursor to the slice it was created from. You
/// cannot hold a `Buff<'a>` after the owning data is dropped or mutated.
///
/// # Examples
/// ```ignore
/// let data = b"\x05\x00\x00\x00hello";
/// let mut buf = Buff::new(data);
///
/// let size = buf.next_u32().unwrap() as usize;  // 5
/// let word = buf.next_bytes(size).unwrap();      // b"hello"
/// ```
pub struct Buff<'a> {
    data: &'a [u8],
    cursor: usize,
}

impl<'a> Buff<'a> {
    // ── Constructors ──────────────────────────────────────────────────────────

    /// Creates a `Buff` over the given byte slice. The cursor starts at 0.
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, cursor: 0 }
    }

    // ── Accessors ─────────────────────────────────────────────────────────────

    /// Returns the full underlying byte slice (regardless of cursor position).
    pub fn as_slice(&self) -> &'a [u8] {
        self.data
    }

    /// Returns the total number of bytes in the buffer.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Returns `true` if the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Returns the current cursor position.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Returns the number of bytes remaining from the cursor to the end.
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.cursor)
    }

    /// Returns `true` if the cursor has reached or passed the end.
    pub fn is_exhausted(&self) -> bool {
        self.cursor >= self.data.len()
    }

    // ── Slicing ───────────────────────────────────────────────────────────────

    /// Returns a sub-slice `[start..end]`, or `None` if out of bounds.
    ///
    /// The returned slice carries the same `'a` lifetime as the original data,
    /// so it remains valid even after the `Buff` cursor moves or is dropped.
    pub fn slice(&self, start: usize, end: usize) -> Option<&'a [u8]> {
        self.data.get(start..end)
    }

    // ── Cursor movement ───────────────────────────────────────────────────────

    /// Resets the cursor to position 0.
    pub fn reset_cursor(&mut self) {
        self.cursor = 0;
    }

    /// Moves the cursor to `pos`, clamped to `[0, len]`.
    pub fn seek(&mut self, pos: usize) {
        self.cursor = pos.min(self.data.len());
    }

    /// Advances the cursor by `n` bytes without reading.
    ///
    /// Returns `false` (leaving the cursor unchanged) if fewer than `n` bytes
    /// remain.
    pub fn skip(&mut self, n: usize) -> bool {
        if self.remaining() < n {
            return false;
        }
        self.cursor += n;
        true
    }

    // ── Sequential reading ────────────────────────────────────────────────────

    /// Reads `size` bytes from the current position and advances the cursor.
    ///
    /// Returns `None` if fewer than `size` bytes remain.
    ///
    /// The returned slice has lifetime `'a` — it points directly into the
    /// original data and is valid for as long as that data lives, independently
    /// of the `Buff` cursor state.
    pub fn next_bytes(&mut self, size: usize) -> Option<&'a [u8]> {
        let start = self.cursor;
        let end = start.checked_add(size)?;
        let bytes = self.data.get(start..end)?;
        self.cursor = end;
        Some(bytes)
    }

    /// Reads a little-endian `u8` and advances the cursor.
    pub fn next_u8(&mut self) -> Option<u8> {
        let bytes = self.next_bytes(1)?;
        Some(bytes[0])
    }

    /// Reads a little-endian `u16` and advances the cursor.
    pub fn next_u16(&mut self) -> Option<u16> {
        let bytes = self.next_bytes(2)?;
        Some(u16::from_le_bytes(bytes.try_into().unwrap()))
    }

    /// Reads a little-endian `u32` and advances the cursor.
    pub fn next_u32(&mut self) -> Option<u32> {
        let bytes = self.next_bytes(4)?;
        Some(u32::from_le_bytes(bytes.try_into().unwrap()))
    }

    /// Reads a little-endian `u64` and advances the cursor.
    pub fn next_u64(&mut self) -> Option<u64> {
        let bytes = self.next_bytes(8)?;
        Some(u64::from_le_bytes(bytes.try_into().unwrap()))
    }

    /// Reads a length-prefixed UTF-8 string.
    ///
    /// Format: `u32` byte-length followed by that many UTF-8 bytes.
    /// Returns `None` if the buffer is exhausted or the bytes are not valid
    /// UTF-8.
    ///
    /// Unlike other `next_*` methods, this must allocate a `String` because
    /// UTF-8 validation requires ownership. If you know the data is valid
    /// UTF-8 and want a zero-copy `&str`, use [`next_str`](Self::next_str).
    pub fn next_string(&mut self) -> Option<String> {
        let s = self.next_str()?;
        Some(s.to_owned())
    }

    /// Reads a length-prefixed UTF-8 string as a borrowed `&str`.
    ///
    /// Returns `None` if the buffer is exhausted or the bytes are not valid
    /// UTF-8. The returned `&str` borrows directly from the underlying data
    /// with lifetime `'a`.
    pub fn next_str(&mut self) -> Option<&'a str> {
        let size = self.next_u32()? as usize;
        let bytes = self.next_bytes(size)?;
        std::str::from_utf8(bytes).ok()
    }
}

// ── Standard trait impls ──────────────────────────────────────────────────────

impl<'a> std::fmt::Debug for Buff<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Buff")
            .field("len", &self.data.len())
            .field("cursor", &self.cursor)
            .finish()
    }
}

impl<'a> From<&'a [u8]> for Buff<'a> {
    fn from(data: &'a [u8]) -> Self {
        Self::new(data)
    }
}

impl<'a> From<&'a Vec<u8>> for Buff<'a> {
    fn from(data: &'a Vec<u8>) -> Self {
        Self::new(data.as_slice())
    }
}

impl<'a> AsRef<[u8]> for Buff<'a> {
    fn as_ref(&self) -> &[u8] {
        self.data
    }
}
