#![allow(clippy::manual_map)]

use fastblur::gaussian_blur;
use image::{imageops, Pixel, GenericImageView, io::Reader as ImageReader, RgbImage, ImageBuffer, Rgb};
use pyo3::prelude::*;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};
use imageproc::drawing::{draw_filled_circle_mut, Blend};
use imageproc::rect::Rect;
use image::DynamicImage::ImageRgb8;
use image::DynamicImage;


mod gradient;
mod noise;
mod resize;

const DEFAULT_WALLPAPER_PATH: &str = "images/default_wallpaper.jpg";
const GENERATED_WALLPAPER_PATH: &str = "images/generated_wallpaper.png";

// Define module
#[pymodule]
fn albumpaper_rs(_py: Python, module: &PyModule) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(generate_save_wallpaper, module)?)?;
    Ok(())
}

type Color = [u8; 3];

// Parameters used in all of these functions
#[derive(FromPyObject, Hash, PartialEq, Eq, Clone)]
pub struct RequiredArgs {
    background_type: String,
    foreground: Foreground,
    display_geometry: [u32; 2],
    available_geometry: [u32; 4],
}

#[derive(FromPyObject, Hash, PartialEq, Eq, Clone)]
pub struct Foreground {
    artwork_buffer: Vec<u8>,
    artwork_size: [u32; 2],
    artwork_resize: u32,
    visable: bool,
}

#[derive(FromPyObject, Hash, PartialEq, Eq, Clone)]
pub struct OptionalArgs {
    blur_radius: Option<u32>,
    color1: Option<Color>,
    color2: Option<Color>,
}

#[pyfunction]
pub fn generate_save_wallpaper(
    _py: Python,
    required_args: RequiredArgs,
    optional_args: OptionalArgs,
) {
    let image = generate_wallpaper(required_args, optional_args);
    image.save(GENERATED_WALLPAPER_PATH).unwrap();
}

// In seperate function for possible caching
fn generate_wallpaper(required_args: RequiredArgs, optional_args: OptionalArgs) -> RgbImage {
    let artwork = RgbImage::from_raw(
        required_args.foreground.artwork_size[0],
        required_args.foreground.artwork_size[1],
        required_args.foreground.artwork_buffer.clone(),
    )
    .unwrap();

    let background = match &required_args.background_type[..] {
        "DefaultWallpaper" => {
            let default_wallpaper = ImageReader::open(DEFAULT_WALLPAPER_PATH)
                .unwrap()
                .decode()
                .unwrap()
                .into_rgb8();
            image_background(
                &default_wallpaper,
                required_args.display_geometry,
                optional_args.blur_radius,
            )
        }
        "Artwork" => image_background(
            &artwork,
            required_args.display_geometry,
            optional_args.blur_radius,
        ),
        "SolidColor" => RgbImage::from_pixel(
            required_args.display_geometry[0],
            required_args.display_geometry[1],
            image::Rgb(optional_args.color1.unwrap()),
        ),
        "LinearGradient" => gradient::linear(
            required_args.display_geometry,
            optional_args.color1.unwrap(),
            optional_args.color2.unwrap(),
        ),
        "RadialGradient" => gradient::radial(
            required_args.display_geometry,
            optional_args.color1.unwrap(),
            optional_args.color2.unwrap(),
            required_args.foreground.artwork_resize,
        ),
        "ColoredNoise" => {
            let mut hasher = DefaultHasher::new();
            required_args.foreground.artwork_buffer.hash(&mut hasher);
            let seed: u32 = hasher.finish() as u32;
            let background = noise::colored(
                required_args.display_geometry,
                optional_args.color1.unwrap(),
                optional_args.color2.unwrap(),
                seed,
            );

            if let Some(blur_radius) = optional_args.blur_radius {
                let [width, height] = required_args.display_geometry;
                add_blur(&background, width, height, blur_radius as f32)
            } else {
                background
            }
        }
        unknown => panic!("Unknown background type '{unknown}'"),
    };
    let artwork_resized = if required_args.foreground.visable {
        Some(resize::fast_resize(
            &artwork,
            required_args.foreground.artwork_resize,
            required_args.foreground.artwork_resize,
        ))
    } else {
        None
    };

    paste_images(
        &background,
        artwork_resized,
        required_args.display_geometry,
        required_args.available_geometry,
    )
}

// Used by wallpaper and artwork backgrounds
fn image_background(
    background: &RgbImage,
    display_geometry: [u32; 2],
    blur_radius: Option<u32>,
) -> RgbImage {
    if let Some(blur) = blur_radius {
        add_blur(
            background,
            display_geometry[0],
            display_geometry[1],
            blur as f32,
        )
    } else {
        resize::fast_resize(background, display_geometry[0], display_geometry[1])
    }
}

fn add_blur(image: &RgbImage, nwidth: u32, nheight: u32, blur_radius: f32) -> RgbImage {
    // Downsize the image by a factor of `scale` for faster blurring and upscale back
    // to display_geometry for final image

    let scale = 4;
    let (scaled_width, scaled_height) = (nwidth / scale, nheight / scale);
    let downscaled_image = resize::fast_resize(image, scaled_width, scaled_height);

    let mut pixels: Vec<[u8; 3]> = downscaled_image
        .into_raw()
        .chunks_exact(3)
        .map(|pixel| pixel.try_into().unwrap())
        .collect();

    gaussian_blur(
        &mut pixels,
        scaled_width.try_into().unwrap(),
        scaled_height.try_into().unwrap(),
        blur_radius / scale as f32,
    );

    let buf = pixels.into_iter().flatten().collect();
    let blurred_image = RgbImage::from_raw(scaled_width, scaled_height, buf).unwrap();
    resize::fast_resize(&blurred_image, nwidth, nheight)
}

fn paste_images(
    background: &RgbImage,
    foreground: Option<RgbImage>,
    display_geometry: [u32; 2],
    available_geometry: [u32; 4],
) -> RgbImage {
    let mut base = RgbImage::new(display_geometry[0], display_geometry[1]);

    // Background Paste
    let x = (display_geometry[0] as i64 - background.width() as i64) / 2;
    let y = (display_geometry[1] as i64 - background.height() as i64) / 2;

    imageops::overlay(&mut base, background, x, y);

    // Foreground paste
    if let Some(mut foreground) = foreground {
        let mut mask = RgbImage::new(foreground.width(), foreground.height());

        // Draw a filled circle on the mask
        let center_x = (foreground.width() / 2) as i32;
        let center_y = (foreground.height() / 2) as i32;
        let radius = std::cmp::min(center_x, center_y);
        let white = Rgb([255u8, 255u8, 255u8]);
        draw_filled_circle_mut(&mut mask, (center_x, center_y), radius, white);

        // Convert the foreground image to RgbaImage
        let foreground_dynamic = ImageRgb8(foreground.clone());
        let mut foreground_rgba = foreground_dynamic.to_rgba8();

        // Apply the mask to the foreground image
        for (x, y, pixel) in mask.enumerate_pixels() {
            if *pixel == Rgb([0, 0, 0]) {
                let mut p = *foreground_rgba.get_pixel(x, y);
                p.channels_mut()[3] = 0;
                foreground_rgba.put_pixel(x, y, p);
            }
        }

        // Convert the base image to RgbaImage
        let base_dynamic = ImageRgb8(base.clone());
        let mut base_rgba = base_dynamic.to_rgba8();

        let x = (i64::from(available_geometry[0]) - i64::from(foreground.width())) / 2
            + i64::from(available_geometry[2]);
        let y = (i64::from(available_geometry[1]) - i64::from(foreground.height())) / 2
            + i64::from(available_geometry[3]);

        imageops::overlay(&mut base_rgba, &foreground_rgba, x, y);

        // Convert RgbaImage back to RgbImage
        let base_dynamic = DynamicImage::ImageRgba8(base_rgba);
        let base_rgb = base_dynamic.to_rgb8();

        base_rgb
    } else {
        // Return a default RgbImage when foreground is None
        RgbImage::new(1, 1)
    }
}