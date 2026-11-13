//! Provider-native profile avatars. RelayHistory owns reading provider files;
//! this module only renders the contract's observed artwork settings.
use super::widgets;
use agent_relay_desktop_core::contract::account::SessionAvatar;
use gtk::{gdk, gdk_pixbuf, glib, prelude::*};
use gtk4 as gtk;
use std::{cell::RefCell, collections::HashMap, rc::Rc, time::Duration};

macro_rules! atlases {
    ($($shape:literal),* $(,)?) => { &[$(($shape, include_bytes!(concat!("../../resources/linux/agents/session-avatar-", $shape, ".png")) as &[u8])),*] };
}
const ART: &[(&str, &[u8])] = atlases!(
    "blob", "pebble", "bean", "egg", "squircle", "tablet", "capsule", "cylinder", "hex", "gem",
    "crystal", "wedge", "shield", "dome", "arch", "cloud", "teardrop", "leaf", "square", "sparkle",
    "clover", "flower", "heart", "house", "star"
);
type Frames = Rc<Vec<gdk::Texture>>;
thread_local! {
    static FRAMES: RefCell<HashMap<(String, String), Frames>> = RefCell::default();
}

fn ink(color: Option<&str>) -> &str {
    match color {
        Some("brown") => "#936439",
        Some("red") => "#FF263C",
        Some("orange") => "#FF6700",
        Some("yellow") => "#FF9800",
        Some("green") => "#00C972",
        Some("cyan") => "#00BCA6",
        Some("blue") => "#1084FE",
        Some("violet") => "#9159FE",
        Some("magenta") => "#FF309B",
        Some("gray") => "#777777",
        _ => widgets::color("text"),
    }
}

fn frames(avatar: &SessionAvatar) -> Option<Frames> {
    let shape = avatar.shape.as_deref()?;
    let ink = ink(avatar.color.as_deref());
    let key = (shape.to_string(), ink.to_string());
    if let Some(frames) = FRAMES.with(|cache| cache.borrow().get(&key).cloned()) {
        return Some(frames);
    }
    let (_, bytes) = ART.iter().find(|(name, _)| *name == shape)?;
    let loader = gdk_pixbuf::PixbufLoader::new();
    loader.write(bytes).ok()?;
    loader.close().ok()?;
    let atlas = loader.pixbuf()?;
    if atlas.width() != 512 || atlas.height() != 256 || atlas.n_channels() != 4 {
        return None;
    }
    let rgb = [1, 3, 5]
        .map(|index| u8::from_str_radix(&ink[index..index + 2], 16).ok())
        .into_iter()
        .collect::<Option<Vec<_>>>()?;
    let pixels = atlas.read_pixel_bytes();
    let stride = atlas.rowstride() as usize;
    let frames = Rc::new(
        (0..32)
            .map(|index| {
                let mut data = Vec::with_capacity(48 * 48 * 4);
                for y in 0..48 {
                    for x in 0..48 {
                        let offset =
                            (index / 8 * 64 + 8 + y) * stride + (index % 8 * 64 + 8 + x) * 4;
                        data.extend_from_slice(&[rgb[0], rgb[1], rgb[2], pixels[offset + 3]]);
                    }
                }
                let image = gdk_pixbuf::Pixbuf::from_mut_slice(
                    data,
                    gdk_pixbuf::Colorspace::Rgb,
                    true,
                    8,
                    48,
                    48,
                    48 * 4,
                );
                gdk::Texture::for_pixbuf(&image)
            })
            .collect(),
    );
    FRAMES.with(|cache| cache.borrow_mut().insert(key, Rc::clone(&frames)));
    Some(frames)
}

fn animations_enabled() -> bool {
    gtk::Settings::default().is_none_or(|settings| settings.is_gtk_enable_animations())
}

pub fn row_badge(session: &crate::models::LocalSessionRow, size: i32) -> gtk::Widget {
    badge(
        &session.icon_source(),
        session.upload.as_ref().and_then(|row| row.avatar.as_ref()),
        size,
    )
}

pub fn badge(source: &str, avatar: Option<&SessionAvatar>, size: i32) -> gtk::Widget {
    let Some(avatar) = avatar else {
        return widgets::agent_badge(source, size);
    };
    if let Some(picture) = inline_image(avatar.image_data_url.as_deref(), size) {
        return sized_picture(&picture, size);
    }
    let Some(frames) = frames(avatar) else {
        return widgets::agent_badge(source, size);
    };
    let picture = gtk::Picture::for_paintable(&frames[0]);
    picture.set_can_shrink(true);
    picture.set_size_request(size, size);
    let weak = picture.downgrade();
    let mut index = 0;
    glib::timeout_add_local(Duration::from_millis(125), move || {
        let Some(picture) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        if picture.is_mapped() {
            index = if animations_enabled() {
                (index + 1) % frames.len()
            } else {
                0
            };
            picture.set_paintable(Some(&frames[index]));
        }
        glib::ControlFlow::Continue
    });
    sized_picture(&picture, size)
}

fn sized_picture(picture: &gtk::Picture, size: i32) -> gtk::Widget {
    let holder = widgets::Sized::new(size, size, picture);
    holder.set_widget_name("session-profile-avatar");
    holder.set_tooltip_text(Some("Grok Bot"));
    widgets::name(&holder, "Grok Bot");
    holder.upcast()
}

fn inline_image(value: Option<&str>, size: i32) -> Option<gtk::Picture> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let value = value.filter(|url| {
        url.len() <= 512 * 1024
            && ["png", "jpeg", "gif", "webp"]
                .iter()
                .any(|format| url.starts_with(&format!("data:image/{format};base64,")))
    })?;
    let bytes = STANDARD.decode(value.split_once(',')?.1).ok()?;
    let loader = gdk_pixbuf::PixbufLoader::new();
    loader.write(&bytes).ok()?;
    loader.close().ok()?;
    let animation = loader.animation()?;
    let picture = gtk::Picture::new();
    picture.set_can_shrink(true);
    picture.set_size_request(size, size);
    let iter = animation.iter(None);
    picture.set_paintable(Some(&gdk::Texture::for_pixbuf(&iter.pixbuf())));
    if !animation.is_static_image() {
        let weak = picture.downgrade();
        glib::timeout_add_local(Duration::from_millis(32), move || {
            let Some(picture) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if picture.is_mapped()
                && animations_enabled()
                && iter.advance(std::time::SystemTime::now())
            {
                picture.set_paintable(Some(&gdk::Texture::for_pixbuf(&iter.pixbuf())));
            }
            glib::ControlFlow::Continue
        });
    }
    Some(picture)
}
