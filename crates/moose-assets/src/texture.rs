use std::path::Path;

use crate::error::LoadError;

/// One mip level of a texture: 32-bit ARGB texels (alpha in the top byte, as the renderer's
/// translucent colors are), row by row from the top. Width and height are powers of two, so
/// texture coordinates wrap (tile) with a mask.
#[derive(Clone, Debug, PartialEq)]
pub struct MipLevel {
    pub width: u32,
    pub height: u32,
    /// log2 of `width` and `height`.
    pub width_log2: u32,
    pub height_log2: u32,
    pub texels: Vec<u32>,
}

impl MipLevel {
    /// The texel at `(x, y)`, wrapping.
    pub fn texel(&self, x: u32, y: u32) -> u32 {
        let (x, y) = (x & (self.width - 1), y & (self.height - 1));
        self.texels[(y * self.width + x) as usize]
    }

    /// The next level down: half the size (each side at least 1), each texel the average of
    /// the 2x2 (or 2x1) texels it covers, in every channel, rounded.
    fn half(&self) -> MipLevel {
        let (w, h) = ((self.width / 2).max(1), (self.height / 2).max(1));
        let (sx, sy) = (self.width / w, self.height / h); // 1 or 2
        let n = sx * sy;
        let texels = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| {
                let mut sum = [0u32; 4];
                for dy in 0..sy {
                    for dx in 0..sx {
                        let t = self.texel(x * sx + dx, y * sy + dy);
                        for (k, s) in sum.iter_mut().enumerate() {
                            *s += (t >> (8 * k)) & 255;
                        }
                    }
                }
                (0..4).fold(0, |c, k| c | ((sum[k] + n / 2) / n) << (8 * k))
            })
            .collect();
        MipLevel {
            width: w,
            height: h,
            width_log2: w.trailing_zeros(),
            height_log2: h.trailing_zeros(),
            texels,
        }
    }
}

/// The faces of a cube map ([`Texture::cube`]), in order: the axis each looks down, and the
/// axes that are right and up in its image, as a camera at the cube's center would see
/// them (a 90 degree square view). Face `i` looks down `+axis` when `i` is even and `-axis`
/// when odd, axes x, y, z in turn. A direction `d` is on the face of its largest component,
/// at `(d·right, d·up) / |d·forward|`, from -1 to 1 across the face.
pub const CUBE_FACES: [[[f32; 3]; 3]; 6] = [
    // forward, right, up
    [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]],
    [[-1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]],
    [[0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
    [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]],
    [[0.0, 0.0, 1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
    [[0.0, 0.0, -1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
];

/// A texture and its mip chain: `levels[0]` is the full image, each next level half the size
/// of the one before (its texels 2x2 averages, alpha included), down to 1x1. Generated when
/// the texture is made. A cube map ([`Texture::cube`]) stacks six faces in each level.
#[derive(Clone, Debug, PartialEq)]
pub struct Texture {
    pub name: String,
    pub levels: Vec<MipLevel>,
}

impl Texture {
    /// A texture from ARGB texels, row by row, with its mip chain. Fails unless both sides
    /// are powers of two and `texels` has exactly `width * height` entries.
    pub fn new(name: &str, width: u32, height: u32, texels: Vec<u32>) -> Result<Self, String> {
        if !width.is_power_of_two() || !height.is_power_of_two() {
            return Err(format!(
                "texture is {width}x{height}; both sides must be powers of two, so it can tile"
            ));
        }
        if texels.len() != (width * height) as usize {
            return Err(format!(
                "{} texels for a {width}x{height} texture",
                texels.len()
            ));
        }
        let mut levels = vec![MipLevel {
            width,
            height,
            width_log2: width.trailing_zeros(),
            height_log2: height.trailing_zeros(),
            texels,
        }];
        while levels.last().is_some_and(|l| l.width > 1 || l.height > 1) {
            let next = levels.last().unwrap().half();
            levels.push(next);
        }
        Ok(Self {
            name: name.to_string(),
            levels,
        })
    }

    /// A cube map from six square `size`x`size` faces of ARGB texels, row by row, in the
    /// order of [`CUBE_FACES`], with its mip chain. Each level stacks the six faces from top
    /// to bottom, so it is `size` wide and `6 * size` tall (not a power of two: sample it
    /// with a cube map sampler, never a tiling one), and each face is halved on its own.
    pub fn cube(name: &str, size: u32, faces: [Vec<u32>; 6]) -> Result<Self, String> {
        if !size.is_power_of_two() {
            return Err(format!(
                "cube face is {size}x{size}; size must be a power of two"
            ));
        }
        let mut faces: Vec<Texture> = faces
            .into_iter()
            .map(|texels| Texture::new(name, size, size, texels))
            .collect::<Result<_, _>>()?;
        let levels = (0..faces[0].levels.len())
            .map(|l| {
                let face = &faces[0].levels[l];
                MipLevel {
                    width: face.width,
                    height: 6 * face.height,
                    width_log2: face.width_log2,
                    height_log2: face.height_log2,
                    texels: faces
                        .iter_mut()
                        .flat_map(|f| std::mem::take(&mut f.levels[l].texels))
                        .collect(),
                }
            })
            .collect();
        Ok(Self {
            name: name.to_string(),
            levels,
        })
    }

    /// A 1x1 texture of one color.
    pub fn solid(name: &str, argb: u32) -> Self {
        Self::new(name, 1, 1, vec![argb]).unwrap()
    }

    /// The full-size level.
    pub fn base(&self) -> &MipLevel {
        &self.levels[0]
    }

    pub fn width(&self) -> u32 {
        self.base().width
    }

    pub fn height(&self) -> u32 {
        self.base().height
    }

    /// The full-size level's texel at `(x, y)`, wrapping.
    pub fn texel(&self, x: u32, y: u32) -> u32 {
        self.base().texel(x, y)
    }
}

/// Decodes a PNG (any color type or bit depth; 16-bit channels are cut to 8, images without
/// alpha get alpha 255) into a [`Texture`].
pub fn decode_png(path: &Path, name: &str, bytes: &[u8]) -> Result<Texture, LoadError> {
    let err = |msg: String| LoadError::new(path, None, msg);
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder
        .read_info()
        .map_err(|e| err(format!("invalid PNG: {e}")))?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| err("PNG too large".into()))?;
    let mut buf = vec![0u8; size];
    let frame = reader
        .next_frame(&mut buf)
        .map_err(|e| err(format!("invalid PNG: {e}")))?;
    let channels = match frame.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        other => return Err(err(format!("unsupported PNG color type {other:?}"))),
    };
    let (w, h) = (frame.width, frame.height);
    let mut texels = Vec::with_capacity((w * h) as usize);
    for y in 0..h as usize {
        let row = &buf[y * frame.line_size..];
        for x in 0..w as usize {
            let p = &row[x * channels..(x + 1) * channels];
            let (r, g, b, a) = match channels {
                1 => (p[0], p[0], p[0], 255),
                2 => (p[0], p[0], p[0], p[1]),
                3 => (p[0], p[1], p[2], 255),
                _ => (p[0], p[1], p[2], p[3]),
            };
            texels.push(u32::from_be_bytes([a, r, g, b]));
        }
    }
    Texture::new(name, w, h, texels).map_err(err)
}
