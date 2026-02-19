use std::{io::Read, time::Duration};

use xcursor::{
    CursorTheme,
    parser::{Image, parse_xcursor},
};

pub struct Cursor {
    icons: Vec<Image>,
    size: u32,
}

impl Cursor {
    pub fn load() -> Self {
        let name = std::env::var("XCURSOR_THEME").unwrap_or_else(|_| "default".into());
        let size = std::env::var("XCURSOR_SIZE").ok().and_then(|s| s.parse().ok()).unwrap_or(24);

        let theme = CursorTheme::load(&name);
        let icons = load_icon(&theme).unwrap_or_else(|| vec![fallback_cursor()]);

        Self { icons, size }
    }

    pub fn get_image(&self, scale: u32, time: Duration) -> Image {
        let size = self.size * scale;
        let millis = u32::try_from(time.as_millis()).unwrap_or(u32::MAX);
        frame(millis, size, &self.icons)
    }
}

fn u32_to_i64(value: u32) -> i64 {
    i64::from(value)
}

fn fallback_cursor() -> Image {
    let width = 24u32;
    let height = 24u32;
    let mut rgba = vec![0u8; (width * height * 4) as usize];

    for y in 0..height {
        for x in 0..width {
            let idx = ((y * width + x) * 4) as usize;
            let is_border = x == 0 || y == 0 || x == width - 1 || y == height - 1;
            let is_diag = x == y || x + 1 == y || y + 1 == x;
            if is_border || is_diag {
                rgba[idx] = 255;
                rgba[idx + 1] = 255;
                rgba[idx + 2] = 255;
                rgba[idx + 3] = 255;
            }
        }
    }

    Image {
        size: 24,
        width,
        height,
        xhot: 1,
        yhot: 1,
        delay: 1,
        pixels_rgba: rgba,
        pixels_argb: vec![],
    }
}

fn load_icon(theme: &CursorTheme) -> Option<Vec<Image>> {
    let icon_path = theme.load_icon("default")?;
    let mut cursor_file = std::fs::File::open(icon_path).ok()?;
    let mut cursor_data = Vec::new();
    cursor_file.read_to_end(&mut cursor_data).ok()?;
    parse_xcursor(&cursor_data)
}

fn nearest_images(size: u32, images: &[Image]) -> impl Iterator<Item = &Image> {
    let nearest_image = images
        .iter()
        .min_by_key(|image| (u32_to_i64(size) - u32_to_i64(image.size)).abs())
        .unwrap();

    images.iter().filter(move |image| {
        image.width == nearest_image.width && image.height == nearest_image.height
    })
}

fn frame(mut millis: u32, size: u32, images: &[Image]) -> Image {
    let total = nearest_images(size, images).fold(0, |acc, image| acc + image.delay);
    if total == 0 {
        return nearest_images(size, images).next().unwrap().clone();
    }
    millis %= total;

    for img in nearest_images(size, images) {
        if millis < img.delay {
            return img.clone();
        }
        millis -= img.delay;
    }

    unreachable!()
}
