//! DJ-режим: автоматичне дозаповнення черги схожими треками.
//!
//! Каскад джерел (як у Feishin), але з «якорем»: тематику підбору тримає
//! трек-якір (спершу сідо), а не дрейфуючий поточний. Порядок сходинок:
//! схожі до якоря (Last.fm); схожі до поточного (якщо він не з випадкового
//! фолбеку); пісні артиста якоря; пісні артиста поточного; випадкові пісні.
//! Випадковий фолбек не рухає якір, тож хаотична вставка не збиває стиль
//! наступних підборів. Усі кандидати фільтруються проти id, що вже є
//! в черзі, — повтори виключені.

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

/// Джерело кандидатів останнього refill (для логування/тестів).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DjStep {
    Similar,
    Artist,
    Random,
}

/// Результат `collect`: свіжі треки + джерело + чи якір має переїхати
/// на поточний трек.
#[derive(Debug)]
pub struct CollectOutcome {
    pub tracks: Vec<TrackMeta>,
    pub source: DjStep,
    pub reanchor_current: bool,
}

/// Порядок баз каскаду: якір першим (тримає тематику), поточний — лише
/// якщо він не з random-фолбеку (інакше якір дрейфує по бібліотеці).
/// Другий елемент кортежу — чи ця база є поточним треком.
pub fn bases_for<'a>(
    anchor: Option<&'a TrackMeta>,
    current: &'a TrackMeta,
    current_is_random: bool,
) -> Vec<(&'a TrackMeta, bool)> {
    let mut bases = Vec::new();
    match anchor {
        Some(a) => {
            bases.push((a, false));
            if !current_is_random && a.id != current.id {
                bases.push((current, true));
            }
        }
        None => bases.push((current, true)),
    }
    bases
}

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
    anchor: Option<&TrackMeta>,
    current: &TrackMeta,
    current_is_random: bool,
    exclude: &HashSet<String>,
) -> Result<CollectOutcome> {
    // Сходинка 1: схожі (Last.fm) до кожної бази по черзі.
    for (base, is_current) in bases_for(anchor, current, current_is_random) {
        let similar = to_metas(client.get_similar_songs(&base.id, DJ_SERVER_COUNT)?);
        let similar = filter_dedupe(similar, exclude);
        if !similar.is_empty() {
            return Ok(CollectOutcome {
                tracks: similar,
                source: DjStep::Similar,
                reanchor_current: is_current,
            });
        }
    }
    // Сходинка 2: пісні артиста бази.
    for (base, is_current) in bases_for(anchor, current, current_is_random) {
        if base.artist.is_empty() {
            continue;
        }
        let by_artist = to_metas(client.search3_songs(&base.artist, DJ_SERVER_COUNT)?);
        let by_artist = filter_dedupe(by_artist, exclude);
        if !by_artist.is_empty() {
            return Ok(CollectOutcome {
                tracks: by_artist,
                source: DjStep::Artist,
                reanchor_current: is_current,
            });
        }
    }
    // Сходинка 3: випадкові пісні — якір не рухається.
    let random = to_metas(client.get_random_songs(DJ_SERVER_COUNT)?);
    Ok(CollectOutcome {
        tracks: filter_dedupe(random, exclude),
        source: DjStep::Random,
        reanchor_current: false,
    })
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

    #[test]
    fn bases_current_only_without_anchor() {
        let current = track("c");
        let bases = bases_for(None, &current, false);
        let ids: Vec<_> = bases.iter().map(|(b, _)| b.id.as_str()).collect();
        let cur: Vec<_> = bases.iter().map(|(_, c)| *c).collect();
        assert_eq!(ids, vec!["c"]);
        assert_eq!(cur, vec![true]);
    }

    #[test]
    fn anchor_equals_current_is_deduplicated() {
        let t = track("a");
        let bases = bases_for(Some(&t), &t, false);
        assert_eq!(bases.len(), 1);
        assert_eq!(bases[0].0.id, "a");
        assert!(!bases[0].1);
    }

    #[test]
    fn anchor_plus_current_when_current_not_random() {
        let anchor = track("a");
        let current = track("c");
        let bases = bases_for(Some(&anchor), &current, false);
        let ids: Vec<_> = bases.iter().map(|(b, _)| b.id.as_str()).collect();
        let cur: Vec<_> = bases.iter().map(|(_, c)| *c).collect();
        assert_eq!(ids, vec!["a", "c"]);
        assert_eq!(cur, vec![false, true]);
    }

    #[test]
    fn random_current_does_not_drag_anchor() {
        let anchor = track("a");
        let current = track("r");
        let bases = bases_for(Some(&anchor), &current, true);
        let ids: Vec<_> = bases.iter().map(|(b, _)| b.id.as_str()).collect();
        assert_eq!(ids, vec!["a"]);
    }
}