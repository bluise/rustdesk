//! Minimal BMP writer, so the probe has no image dependency.

use std::fs::File;
use std::io::{self, Write};
use std::mem::size_of;

/// Writes a 32-bit top-down BGRA bitmap: row 0 of `bgra` is the top row.
pub fn write(path: &str, width: usize, height: usize, bgra: &[u8]) -> io::Result<()> {
    assert_eq!(bgra.len(), width * height * 4, "buffer size mismatch");
    let image_size = (bgra.len()) as u32;
    let header_size = size_of::<BitmapInfoHeader>() as u32;

    let mut file = File::create(path)?;
    // BITMAPFILEHEADER (packed on disk; written field by field to avoid
    // repr(packed) games in Rust).
    file.write_all(b"BM")?;
    file.write_all(&(14 + header_size + image_size).to_le_bytes())?;
    file.write_all(&[0u8; 4])?; // reserved
    file.write_all(&(14 + header_size).to_le_bytes())?;

    let info = BitmapInfoHeader {
        size: header_size,
        width: width as i32,
        // Negative height means top-down, matching the frame layout.
        height: -(height as i32),
        planes: 1,
        bit_count: 32,
        compression: 0, // BI_RGB
        image_size,
        x_pels: 0,
        y_pels: 0,
        clr_used: 0,
        clr_important: 0,
    };
    file.write_all(&info.to_bytes())?;
    file.write_all(bgra)?;
    file.flush()
}

struct BitmapInfoHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    image_size: u32,
    x_pels: i32,
    y_pels: i32,
    clr_used: u32,
    clr_important: u32,
}

impl BitmapInfoHeader {
    fn to_bytes(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(size_of::<Self>());
        v.extend_from_slice(&self.size.to_le_bytes());
        v.extend_from_slice(&self.width.to_le_bytes());
        v.extend_from_slice(&self.height.to_le_bytes());
        v.extend_from_slice(&self.planes.to_le_bytes());
        v.extend_from_slice(&self.bit_count.to_le_bytes());
        v.extend_from_slice(&self.compression.to_le_bytes());
        v.extend_from_slice(&self.image_size.to_le_bytes());
        v.extend_from_slice(&self.x_pels.to_le_bytes());
        v.extend_from_slice(&self.y_pels.to_le_bytes());
        v.extend_from_slice(&self.clr_used.to_le_bytes());
        v.extend_from_slice(&self.clr_important.to_le_bytes());
        v
    }
}