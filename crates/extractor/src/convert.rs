//! Conversions from the SWF parser's types to the extracted format's.

use bb_format as f;
use swf::{FillStyle, SwfStr, Twips};

pub fn px(twips: Twips) -> f64 {
    f64::from(twips.get()) / 20.0
}

pub fn matrix(m: &swf::Matrix) -> f::Matrix {
    [
        m.a.to_f64(),
        m.b.to_f64(),
        m.c.to_f64(),
        m.d.to_f64(),
        px(m.tx),
        px(m.ty),
    ]
}

pub fn color(c: &swf::Color) -> f::Color {
    f::Color {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
}

pub fn rect(r: &swf::Rectangle<Twips>) -> f::Rect {
    f::Rect {
        x_min: px(r.x_min),
        y_min: px(r.y_min),
        x_max: px(r.x_max),
        y_max: px(r.y_max),
    }
}

pub fn color_transform(c: &swf::ColorTransform) -> f::ColorTransform {
    f::ColorTransform {
        mult: [
            c.r_multiply.to_f64(),
            c.g_multiply.to_f64(),
            c.b_multiply.to_f64(),
            c.a_multiply.to_f64(),
        ],
        add: [c.r_add, c.g_add, c.b_add, c.a_add],
    }
}

pub fn filter(filter: &swf::Filter) -> f::Filter {
    let unsupported = |name: &str| f::Filter::Unsupported {
        name: name.to_owned(),
    };
    match filter {
        swf::Filter::BlurFilter(blur) => f::Filter::Blur {
            blur_x: blur.blur_x.to_f64(),
            blur_y: blur.blur_y.to_f64(),
            passes: blur.num_passes(),
        },
        swf::Filter::DropShadowFilter(_) => unsupported("drop_shadow"),
        swf::Filter::GlowFilter(_) => unsupported("glow"),
        swf::Filter::BevelFilter(_) => unsupported("bevel"),
        swf::Filter::GradientGlowFilter(_) => unsupported("gradient_glow"),
        swf::Filter::ConvolutionFilter(_) => unsupported("convolution"),
        swf::Filter::ColorMatrixFilter(_) => unsupported("color_matrix"),
        swf::Filter::GradientBevelFilter(_) => unsupported("gradient_bevel"),
    }
}

pub fn paint(style: &FillStyle) -> f::Paint {
    let stops = |gradient: &swf::Gradient| {
        gradient
            .records
            .iter()
            .map(|record| f::GradientStop {
                offset: f64::from(record.ratio) / 255.0,
                color: color(&record.color),
            })
            .collect()
    };
    match style {
        FillStyle::Color(c) => f::Paint::Solid { color: color(c) },
        FillStyle::LinearGradient(gradient) => f::Paint::LinearGradient {
            matrix: matrix(&gradient.matrix),
            stops: stops(gradient),
        },
        FillStyle::RadialGradient(gradient) => f::Paint::RadialGradient {
            matrix: matrix(&gradient.matrix),
            stops: stops(gradient),
            focal: 0.0,
        },
        FillStyle::FocalGradient {
            gradient,
            focal_point,
        } => f::Paint::RadialGradient {
            matrix: matrix(&gradient.matrix),
            stops: stops(gradient),
            focal: focal_point.to_f64(),
        },
        FillStyle::Bitmap {
            id,
            matrix: m,
            is_smoothed,
            is_repeating,
        } => f::Paint::Bitmap {
            bitmap: *id,
            // The file's matrix produces twips; ours produce pixels.
            matrix: [
                m.a.to_f64() / 20.0,
                m.b.to_f64() / 20.0,
                m.c.to_f64() / 20.0,
                m.d.to_f64() / 20.0,
                px(m.tx),
                px(m.ty),
            ],
            smoothed: *is_smoothed,
            repeating: *is_repeating,
        },
    }
}

pub fn sound_start(sound: u16, info: &swf::SoundInfo) -> f::SoundStart {
    f::SoundStart {
        sound,
        event: match info.event {
            swf::SoundEvent::Event => f::SoundEvent::Event,
            swf::SoundEvent::Start => f::SoundEvent::Start,
            swf::SoundEvent::Stop => f::SoundEvent::Stop,
        },
        loops: info.num_loops,
        in_sample: info.in_sample,
        out_sample: info.out_sample,
        envelope: info
            .envelope
            .iter()
            .flatten()
            .map(|point| f::EnvelopePoint {
                sample: point.sample,
                left: point.left_volume,
                right: point.right_volume,
            })
            .collect(),
    }
}

/// Decodes the file's strings, whose encoding depends on its version.
#[derive(Clone, Copy)]
pub struct Strings {
    pub swf_version: u8,
}

impl Strings {
    pub fn get(&self, s: &SwfStr) -> String {
        s.to_string_lossy(SwfStr::encoding_for_version(self.swf_version))
    }
}
