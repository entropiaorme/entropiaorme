//! The image buffer every reader of the game's screen works on, and the
//! digit values the recogniser can emit.

/// One BGR HWC image buffer.
#[derive(Debug, Clone, PartialEq)]
pub struct BgrImage {
    pub data: Vec<u8>,
    pub h: usize,
    pub w: usize,
}

impl BgrImage {
    /// The rectangle `[y0, y1) x [x0, x1)`, clamped into bounds: a
    /// rectangle reaching past an edge is cut at it, and a negative
    /// coordinate clamps to zero.
    pub fn crop(&self, y0: i64, y1: i64, x0: i64, x1: i64) -> BgrImage {
        let y0 = y0.clamp(0, self.h as i64) as usize;
        let y1 = y1.clamp(y0 as i64, self.h as i64) as usize;
        let x0 = x0.clamp(0, self.w as i64) as usize;
        let x1 = x1.clamp(x0 as i64, self.w as i64) as usize;
        let (ch, cw) = (y1 - y0, x1 - x0);
        let mut data = Vec::with_capacity(ch * cw * 3);
        for y in y0..y1 {
            let start = (y * self.w + x0) * 3;
            data.extend_from_slice(&self.data[start..start + cw * 3]);
        }
        BgrImage { data, h: ch, w: cw }
    }
}

/// The decimal value of a digit character the recogniser can emit: its
/// decode alphabet carries exactly the ASCII and fullwidth digit forms,
/// and a number read in either converts by value.
pub(crate) fn digit_value(ch: char) -> Option<u32> {
    match ch {
        '0'..='9' => Some(ch as u32 - '0' as u32),
        '\u{ff10}'..='\u{ff19}' => Some(ch as u32 - 0xff10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_and_fullwidth_digits_convert_by_value() {
        assert_eq!(digit_value('0'), Some(0));
        assert_eq!(digit_value('7'), Some(7));
        assert_eq!(digit_value('\u{ff10}'), Some(0));
        assert_eq!(digit_value('\u{ff19}'), Some(9));
        for other in ['a', ' ', ',', '.', '\u{0660}', '\u{ff1a}'] {
            assert_eq!(digit_value(other), None, "{other:?}");
        }
    }

    #[test]
    fn a_crop_is_cut_at_the_image_edges() {
        // A 3x2 image whose pixels carry their own coordinates.
        let mut data = Vec::new();
        for y in 0..2u8 {
            for x in 0..3u8 {
                data.extend_from_slice(&[x, y, 0]);
            }
        }
        let image = BgrImage { data, h: 2, w: 3 };
        let inner = image.crop(1, 2, 1, 3);
        assert_eq!((inner.w, inner.h), (2, 1));
        assert_eq!(inner.data, vec![1, 1, 0, 2, 1, 0]);
        let overrun = image.crop(-5, 9, 2, 9);
        assert_eq!((overrun.w, overrun.h), (1, 2));
        assert_eq!(overrun.data, vec![2, 0, 0, 2, 1, 0]);
        let empty = image.crop(1, 0, 0, 3);
        assert_eq!((empty.w, empty.h, empty.data.len()), (3, 0, 0));
    }
}
