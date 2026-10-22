//! Coarse colour comparison, not a pixel alignment or perceptual similarity metric.
//! RGB means are 0–255 sRGB; luma is 0–255 Rec. 709 weighted sRGB (not linear light).

use std::path::Path;

use anyhow::{Context, Result, ensure};
use image::{Rgb, RgbImage, imageops};

const GAP: u32 = 8;
const GAP_COLOUR: Rgb<u8> = Rgb([48, 48, 48]);

pub fn run(reference: &Path, ours: &Path, out: &Path, height: u32) -> Result<()> {
    let load = |path: &Path| -> Result<RgbImage> {
        Ok(image::open(path)
            .with_context(|| format!("read {}", path.display()))?
            .to_rgb8())
    };
    let reference = scale(&load(reference)?, height)?;
    let ours = scale(&load(ours)?, height)?;
    let ref_mean = central_mean(&reference);
    let ours_mean = central_mean(&ours);
    print_mean("ref ", ref_mean);
    print_mean("ours", ours_mean);
    print_mean(
        "diff (ours - ref)",
        std::array::from_fn(|i| ours_mean[i] - ref_mean[i]),
    );
    side_by_side(&reference, &ours)?
        .save_with_format(out, image::ImageFormat::Png)
        .with_context(|| format!("write {}", out.display()))?;
    Ok(())
}

fn scale(image: &RgbImage, height: u32) -> Result<RgbImage> {
    ensure!(
        height > 0 && image.width() > 0 && image.height() > 0,
        "image dimensions must be positive"
    );
    let width = (u64::from(image.width()) * u64::from(height) + u64::from(image.height()) / 2)
        / u64::from(image.height());
    let width = u32::try_from(width.max(1)).context("scaled image is too wide")?;
    Ok(imageops::resize(
        image,
        width,
        height,
        imageops::FilterType::Lanczos3,
    ))
}

fn side_by_side(reference: &RgbImage, ours: &RgbImage) -> Result<RgbImage> {
    ensure!(
        reference.height() == ours.height(),
        "images must have the same height"
    );
    let width = reference
        .width()
        .checked_add(GAP)
        .and_then(|w| w.checked_add(ours.width()))
        .context("comparison is too wide")?;
    let mut combined = RgbImage::from_pixel(width, reference.height(), GAP_COLOUR);
    imageops::replace(&mut combined, reference, 0, 0);
    imageops::replace(
        &mut combined,
        ours,
        i64::from(reference.width()) + i64::from(GAP),
        0,
    );
    Ok(combined)
}

/// Sample pixel centres inside the middle 40% × 60%, with at least one pixel per axis.
/// Measurements use the resized images shown in the comparison.
fn central_mean(image: &RgbImage) -> [f64; 4] {
    let region = |size: u32, fraction: f64| {
        let start = ((f64::from(size) * (1.0 - fraction) * 0.5 - 0.5)
            .ceil()
            .max(0.0) as u32)
            .min(size - 1);
        let end = ((f64::from(size) * (1.0 + fraction) * 0.5 - 0.5).ceil() as u32)
            .min(size)
            .max(start + 1);
        start..end
    };
    let xs = region(image.width(), 0.4);
    let ys = region(image.height(), 0.6);
    let count = f64::from(xs.len() as u32) * f64::from(ys.len() as u32);
    let mut rgb = [0.0; 3];
    for y in ys {
        for x in xs.clone() {
            for (sum, channel) in rgb.iter_mut().zip(image.get_pixel(x, y).0) {
                *sum += f64::from(channel);
            }
        }
    }
    let [r, g, b] = rgb.map(|sum| sum / count);
    [r, g, b, 0.2126 * r + 0.7152 * g + 0.0722 * b]
}

fn print_mean(label: &str, [r, g, b, luma]: [f64; 4]) {
    println!("{label} mean rgb ({r:.2}, {g:.2}, {b:.2}) luma {luma:.2}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaled_comparison_preserves_aspect_and_has_dark_gap() {
        let reference = scale(&RgbImage::from_pixel(4, 2, Rgb([255, 0, 0])), 4).unwrap();
        let ours = scale(&RgbImage::from_pixel(2, 2, Rgb([0, 0, 255])), 4).unwrap();
        let combined = side_by_side(&reference, &ours).unwrap();
        assert_eq!(combined.dimensions(), (8 + GAP + 4, 4));
        for y in 0..4 {
            assert_eq!(*combined.get_pixel(7, y), Rgb([255, 0, 0]));
            for x in 8..8 + GAP {
                assert_eq!(*combined.get_pixel(x, y), GAP_COLOUR);
            }
            assert_eq!(*combined.get_pixel(8 + GAP, y), Rgb([0, 0, 255]));
        }
    }

    #[test]
    fn central_mean_excludes_surroundings_and_uses_srgb_luma() {
        let mut image = RgbImage::from_pixel(10, 10, Rgb([255, 255, 255]));
        for y in 2..8 {
            for x in 3..7 {
                image.put_pixel(x, y, Rgb([100, 50, 0]));
            }
        }
        let [r, g, b, luma] = central_mean(&image);
        assert_eq!([r, g, b], [100.0, 50.0, 0.0]);
        assert!((luma - 57.02).abs() < 1e-9);
        assert_eq!(
            central_mean(&RgbImage::from_pixel(1, 1, Rgb([0, 0, 0]))),
            [0.0; 4]
        );
        assert_eq!(
            central_mean(&RgbImage::from_pixel(4, 2, Rgb([8, 16, 32])))[..3],
            [8.0, 16.0, 32.0]
        );
    }

    #[test]
    fn rejects_zero_height_and_mismatched_comparison_heights() {
        let image = RgbImage::new(4, 2);
        assert!(scale(&image, 0).is_err());
        assert!(side_by_side(&image, &RgbImage::new(4, 3)).is_err());
    }
}
