//! DJ-режим: автоматичне дозаповнення черги схожими треками.
//!
//! Каскад джерел (як у Feishin): схожі до поточного (Last.fm) → пісні
//! того ж артиста → випадкові пісні. Кожна наступна сходинка використовується
//! лише якщо попередня порожня. Всі кандидати фільтруються проти id, що вже
//! є в черзі, — повтори виключені.

use std::collections::HashSet;

use crate::api::client::Client;
use crate::api::models::Child;
use crate::error::Result;
use crate::playback::engine::TrackMeta;

/// Поріг: коли в черзі після поточного залишається менше треків — дозаповнюємо.
pub const DJ_REFILL_AT: usize = 10;
/// Скільки кандидатів додається за одне дозаповнення.
pub const DJ_BATCH: usize = 25;
/// Скільки кандидатів питаємо з сервера. Navidrome повертає порожньо при
/// `count < 14` (перевірено наживо) — тримаємося вище межі.
pub const DJ_SERVER_COUNT: u32 = 50;

/// Відсікає треки з exclude-множини і дублікати, зберігаючи порядок.
pub fn filter_dedupe(
    candidates: Vec<TrackMeta>,
    exclude: &HashSet<String>,
) -> Vec<TrackMeta> {
    let mut seen = HashSet::with_capacity(candidates.len());
    candidates
        .into_iter()
        .filter(|t| !exclude.contains(&t.id) && seen.insert(t.id.clone()))
        .collect()
}

/// Каскадний збір кандидатів для дозаповнення DJ-черги.
pub fn collect(
    client: &Client,
    current: &TrackMeta,
    exclude: &HashSet<String>,
) -> Result<Vec<TrackMeta>> {
    let similar = to_metas(client.get_similar_songs(&current.id, DJ_SERVER_COUNT)?);
    let similar = filter_dedupe(similar, exclude);
    if !similar.is_empty() {
        return Ok(similar);
    }

    if !current.artist.is_empty() {
        let by_artist = to_metas(client.search3_songs(&current.artist, DJ_SERVER_COUNT)?);
        let by_artist = filter_dedupe(by_artist, exclude);
        if !by_artist.is_empty() {
            return Ok(by_artist);
        }
    }

    let random = to_metas(client.get_random_songs(DJ_SERVER_COUNT)?);
    Ok(filter_dedupe(random, exclude))
}

/// Пісні (без директорій) → TrackMeta.
fn to_metas(children: Vec<Child>) -> Vec<TrackMeta> {
    children
        .iter()
        .filter(|c| !c.is_dir.unwrap_or(false))
        .map(TrackMeta::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str) -> TrackMeta {
        TrackMeta {
            id: id.to_string(),
            title: format!("title {id}"),
            artist: "artist".into(),
            album: "album".into(),
            album_id: String::new(),
            cover_art: None,
            duration: 0,
            track_number: None,
            disc_number: None,
        }
    }

    #[test]
    fn filter_dedupes_and_excludes() {
        let candidates = vec![track("a"), track("b"), track("c"), track("a")];
        let exclude: HashSet<_> = ["b".to_string()].into_iter().collect();
        let out = filter_dedupe(candidates, &exclude);
        let ids: Vec<_> = out.iter().map(|t| t.id.clone()).collect();
        assert_eq!(ids, vec!["a", "c"]);
    }

    #[test]
    fn filter_keeps_order_of_unique_tracks() {
        let candidates = vec![track("x"), track("y"), track("z")];
        let out = filter_dedupe(candidates, &HashSet::new());
        let ids: Vec<_> = out.iter().map(|t| t.id.clone()).collect();
        assert_eq!(ids, vec!["x", "y", "z"]);
    }

    #[test]
    fn filter_drops_directories() {
        let dir = Child { id: "dir1".into(), is_dir: Some(true), ..Default::default() };
        let map = to_metas(vec![dir, Child { id: "song1".into(), ..Default::default() }]);
        let ids: Vec<_> = map.iter().map(|t| t.id.clone()).collect();
        assert_eq!(ids, vec!["song1"]);
    }
}