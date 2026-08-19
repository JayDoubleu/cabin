//! Frame buffers handed from the RDP session to the widget.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// Four bytes per pixel, blue first. This is what GDK wants on little
    /// endian, and what the RDP codecs produce.
    Bgra8888,
}

impl PixelFormat {
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Bgra8888 => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// A whole guest framebuffer, plus the regions that changed.
///
/// The first version of the widget uploads the whole buffer each time. The
/// damage list is carried now so that a later switch to sub-uploads needs no
/// change to this contract. See DESIGN.md section 6.2.
#[derive(Debug, Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub data: Vec<u8>,
    pub damage: Vec<Rect>,
}

impl Frame {
    pub fn stride(&self) -> usize {
        self.width as usize * self.format.bytes_per_pixel()
    }

    /// Checks that the buffer length matches the stated size.
    pub fn is_consistent(&self) -> bool {
        self.data.len() == self.stride() * self.height as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: u32, h: u32, len: usize) -> Frame {
        Frame {
            width: w,
            height: h,
            format: PixelFormat::Bgra8888,
            data: vec![0; len],
            damage: Vec::new(),
        }
    }

    #[test]
    fn stride_is_four_bytes_per_pixel() {
        assert_eq!(frame(1920, 1080, 0).stride(), 7680);
    }

    #[test]
    fn detects_a_short_buffer() {
        assert!(frame(4, 4, 64).is_consistent());
        assert!(!frame(4, 4, 63).is_consistent());
    }

    #[test]
    fn empty_rect_is_detected() {
        assert!(Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 8
        }
        .is_empty());
        assert!(!Rect {
            x: 0,
            y: 0,
            width: 8,
            height: 8
        }
        .is_empty());
    }
}
