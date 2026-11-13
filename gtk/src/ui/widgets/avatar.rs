//! Shared, coalesced profile pictures for every message and account row.
use super::{add_classes, clear, hbox, Sized};
use crate::models::initials;
use gtk::{gdk, glib, prelude::*};
use gtk4 as gtk;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

type WaitingAvatar = (glib::WeakRef<gtk::Box>, i32);
thread_local! {
    static AVATARS: RefCell<HashMap<String, gdk::Texture>> = RefCell::default();
    static LOADING: RefCell<HashSet<String>> = RefCell::default();
    static WAITING: RefCell<HashMap<String, Vec<WaitingAvatar>>> = RefCell::default();
}

/// Avatars are at most 2 MiB.
const AVATAR_LIMIT: u64 = 2 * 1024 * 1024;

/// A person's picture (a public `https` image, such as the Gravatar URL the
/// core names), else their initials.
pub fn avatar(person_name: &str, url: Option<&str>, size: i32) -> gtk::Box {
    let size = super::zoom::px(size);
    let holder = hbox(0, &[]);
    holder.set_size_request(size, size);
    holder.set_halign(gtk::Align::Center);
    holder.set_valign(gtk::Align::Center);
    holder.set_overflow(gtk::Overflow::Hidden);
    holder.add_css_class("avatar-picture");
    let fallback = gtk::Label::new(Some(&initials(if person_name.is_empty() {
        "?"
    } else {
        person_name
    })));
    add_classes(&fallback, &["avatar"]);
    fallback.set_size_request(size, size);
    let font = (size as f64 * 0.3).max(7.0) as i32;
    let attributes = gtk::pango::AttrList::new();
    attributes.insert(gtk::pango::AttrSize::new(font * gtk::pango::SCALE));
    attributes.insert(gtk::pango::AttrInt::new_weight(gtk::pango::Weight::Bold));
    fallback.set_attributes(Some(&attributes));
    holder.append(&fallback);
    let Some(url) = url
        .filter(|url| url.starts_with("https://") || url.starts_with("data:image/"))
        .map(str::to_owned)
    else {
        return holder;
    };
    if let Some(texture) = AVATARS.with(|cache| cache.borrow().get(&url).cloned()) {
        show_avatar(&holder, &texture, size);
        return holder;
    }
    WAITING.with(|waiting| {
        let mut waiting = waiting.borrow_mut();
        let rows = waiting.entry(url.clone()).or_default();
        rows.retain(|(row, _)| row.upgrade().is_some());
        rows.push((holder.downgrade(), size));
    });
    if LOADING.with(|loading| loading.borrow_mut().insert(url.clone())) {
        start_download(url);
    }
    holder
}

fn show_avatar(holder: &gtk::Box, texture: &gdk::Texture, size: i32) {
    clear(holder);
    let picture = gtk::Picture::for_paintable(texture);
    picture.set_can_shrink(true);
    picture.set_keep_aspect_ratio(false);
    holder.append(&Sized::new(size, size, &picture));
}

fn start_download(url: String) {
    glib::spawn_future_local(async move {
        let download_url = url.clone();
        let fetched =
            gtk::gio::spawn_blocking(move || retry_download(|| fetch_avatar(&download_url))).await;
        LOADING.with(|loading| loading.borrow_mut().remove(&url));
        let Ok(Some(bytes)) = fetched else {
            prune_waiting(&url);
            return;
        };
        let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)) else {
            prune_waiting(&url);
            return;
        };
        AVATARS.with(|cache| cache.borrow_mut().insert(url.clone(), texture.clone()));
        let rows = WAITING.with(|waiting| waiting.borrow_mut().remove(&url).unwrap_or_default());
        for (row, size) in rows {
            if let Some(row) = row.upgrade() {
                show_avatar(&row, &texture, size);
            }
        }
    });
}

fn prune_waiting(url: &str) {
    WAITING.with(|waiting| {
        let mut waiting = waiting.borrow_mut();
        if let Some(rows) = waiting.get_mut(url) {
            rows.retain(|(row, _)| row.upgrade().is_some());
        }
        waiting.retain(|_, rows| !rows.is_empty());
    });
}

fn retry_download(mut fetch: impl FnMut() -> Option<Vec<u8>>) -> Option<Vec<u8>> {
    let first = fetch();
    if first.is_some() {
        return first;
    }
    std::thread::sleep(Duration::from_millis(250));
    fetch()
}

/// The picture's bytes, when it stays on `https` and fits the limit.
fn fetch_avatar(url: &str) -> Option<Vec<u8>> {
    use std::io::Read;
    if url.starts_with("data:image/") {
        use base64::Engine;
        let (header, data) = url.split_once(',')?;
        if ![
            "data:image/png;base64",
            "data:image/jpeg;base64",
            "data:image/webp;base64",
            "data:image/gif;base64",
        ]
        .contains(&header)
            || data.len() as u64 > AVATAR_LIMIT * 4 / 3 + 4
        {
            return None;
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .ok()?;
        return (bytes.len() as u64 <= AVATAR_LIMIT).then_some(bytes);
    }
    let response = ureq::get(url)
        .set("User-Agent", "AgentRelayDesktop")
        .timeout(Duration::from_secs(10))
        .call()
        .ok()?;
    if !response.get_url().starts_with("https://") {
        return None;
    }
    let mut data = Vec::new();
    response
        .into_reader()
        .take(AVATAR_LIMIT + 1)
        .read_to_end(&mut data)
        .ok()?;
    (data.len() as u64 <= AVATAR_LIMIT).then_some(data)
}

#[cfg(test)]
mod tests {
    #[test]
    fn transient_avatar_download_failure_retries_once() {
        let mut calls = 0;
        let bytes = super::retry_download(|| {
            calls += 1;
            (calls == 2).then(|| vec![1, 2, 3])
        });
        assert_eq!(calls, 2);
        assert_eq!(bytes, Some(vec![1, 2, 3]));
    }

    #[test]
    fn inline_workspace_avatar_images_are_bounded_and_typed() {
        let png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jB1sAAAAASUVORK5CYII=";
        assert!(super::fetch_avatar(png).unwrap().starts_with(b"\x89PNG"));
        assert!(super::fetch_avatar("data:image/svg+xml;base64,PHN2Zz4=").is_none());
        assert!(super::fetch_avatar("data:image/png;base64,invalid!").is_none());
        assert!(super::fetch_avatar(&format!(
            "data:image/png;base64,{}",
            "A".repeat(super::AVATAR_LIMIT as usize * 2)
        ))
        .is_none());
    }
}
