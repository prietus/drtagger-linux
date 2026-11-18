//! Artwork sources: the Cover Art Archive (images attached to a
//! MusicBrainz release), fanart.tv (by release group, needs a key), and
//! the iTunes and Deezer album searches (no key).

use crate::error::{ProviderError, Result};
use crate::http::{Request, Transport, default_transport, encode_path, with_query};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

fn get(transport: &Arc<dyn Transport>, url: &str, user_agent: &str, json: bool) -> Result<crate::http::Response> {
    let mut r = Request::get(url).header("User-Agent", user_agent.to_string());
    if json {
        r = r.header("Accept", "application/json");
    }
    transport.send(&r)
}

/// `URL(string:)` of a provider string: empty means none.
fn url(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.is_empty())
}

// MARK: - Cover Art Archive

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CAAImage {
    pub id: String,
    pub is_front: bool,
    pub is_back: bool,
    pub types: Vec<String>,
    #[serde(rename = "fullURL")]
    pub full_url: String,
    /// 1200 px.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub large: Option<String>,
    /// 500 px.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub small: Option<String>,
    /// 250 px.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

pub struct CoverArtArchiveClient {
    pub user_agent: String,
    transport: Arc<dyn Transport>,
}

impl CoverArtArchiveClient {
    pub fn new(user_agent: &str) -> CoverArtArchiveClient {
        CoverArtArchiveClient { user_agent: user_agent.into(), transport: default_transport() }
    }

    /// Empty when the release has no art (404, which is common).
    pub fn images(&self, release_id: &str) -> Result<Vec<CAAImage>> {
        let r = get(
            &self.transport,
            &format!("https://coverartarchive.org/release/{}", encode_path(release_id)),
            &self.user_agent,
            true,
        )?;
        match r.status {
            404 => Ok(vec![]),
            s if s >= 400 => Err(ProviderError::Http(s)),
            _ => Self::parse(&r.body),
        }
    }

    pub fn front_url(release_id: &str, size: u32) -> String {
        format!("https://coverartarchive.org/release/{release_id}/front-{size}")
    }

    pub fn parse(data: &[u8]) -> Result<Vec<CAAImage>> {
        #[derive(Deserialize)]
        struct Root {
            images: Vec<Img>,
        }
        #[derive(Deserialize)]
        struct Img {
            id: Option<i64>,
            image: String,
            front: Option<bool>,
            back: Option<bool>,
            types: Option<Vec<String>>,
            comment: Option<String>,
            thumbnails: Option<HashMap<String, String>>,
        }
        let root: Root = serde_json::from_slice(data)?;
        Ok(root
            .images
            .into_iter()
            .filter_map(|img| {
                let full = url(Some(img.image))?;
                let thumb = |k: &str| url(img.thumbnails.as_ref().and_then(|t| t.get(k).cloned()));
                Some(CAAImage {
                    id: img.id.map(|i| i.to_string()).unwrap_or_default(),
                    is_front: img.front.unwrap_or(false),
                    is_back: img.back.unwrap_or(false),
                    types: img.types.clone().unwrap_or_default(),
                    full_url: full,
                    large: thumb("1200"),
                    small: thumb("500"),
                    thumbnail: thumb("250"),
                    comment: img.comment.filter(|c| !c.is_empty()),
                })
            })
            .collect())
    }
}

// MARK: - Deezer

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeezerAlbum {
    pub id: i64,
    pub artist: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_count: Option<i64>,
    /// 1000 px.
    #[serde(rename = "coverXL", default, skip_serializing_if = "Option::is_none")]
    pub cover_xl: Option<String>,
    /// 500 px.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_big: Option<String>,
}

pub struct DeezerClient {
    pub user_agent: String,
    transport: Arc<dyn Transport>,
}

impl DeezerClient {
    pub fn new(user_agent: &str) -> DeezerClient {
        DeezerClient { user_agent: user_agent.into(), transport: default_transport() }
    }

    pub fn search_albums(&self, artist: &str, album: &str, limit: usize) -> Result<Vec<DeezerAlbum>> {
        let mut parts = vec![];
        if !artist.is_empty() {
            parts.push(format!("artist:\"{artist}\""));
        }
        if !album.is_empty() {
            parts.push(format!("album:\"{album}\""));
        }
        if parts.is_empty() {
            return Ok(vec![]);
        }
        let u = with_query(
            "https://api.deezer.com/search/album",
            &[("q", &parts.join(" ")), ("limit", &limit.to_string())],
        );
        let r = get(&self.transport, &u, &self.user_agent, false)?;
        if r.status >= 400 {
            return Err(ProviderError::Http(r.status));
        }
        Self::parse(&r.body)
    }

    pub fn parse(data: &[u8]) -> Result<Vec<DeezerAlbum>> {
        #[derive(Deserialize)]
        struct Root {
            data: Vec<Album>,
        }
        #[derive(Deserialize)]
        struct Album {
            id: i64,
            title: Option<String>,
            nb_tracks: Option<i64>,
            cover_xl: Option<String>,
            cover_big: Option<String>,
            artist: Option<Artist>,
        }
        #[derive(Deserialize)]
        struct Artist {
            name: Option<String>,
        }
        let root: Root = serde_json::from_slice(data)?;
        Ok(root
            .data
            .into_iter()
            .map(|a| DeezerAlbum {
                id: a.id,
                artist: a.artist.and_then(|x| x.name).unwrap_or_default(),
                title: a.title.unwrap_or_default(),
                track_count: a.nb_tracks,
                cover_xl: url(a.cover_xl),
                cover_big: url(a.cover_big),
            })
            .collect())
    }
}

// MARK: - iTunes

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ITunesAlbum {
    #[serde(rename = "collectionID")]
    pub collection_id: i64,
    pub artist: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(rename = "artworkURL100")]
    pub artwork_url_100: String,
}

impl ITunesAlbum {
    pub fn artwork_url_3000(&self) -> String {
        ITunesSearchClient::resize(&self.artwork_url_100, 3000)
    }

    pub fn artwork_url_1500(&self) -> String {
        ITunesSearchClient::resize(&self.artwork_url_100, 1500)
    }
}

pub struct ITunesSearchClient {
    pub user_agent: String,
    transport: Arc<dyn Transport>,
}

impl ITunesSearchClient {
    pub fn new(user_agent: &str) -> ITunesSearchClient {
        ITunesSearchClient { user_agent: user_agent.into(), transport: default_transport() }
    }

    pub fn search_albums(&self, artist: &str, album: &str, country: &str, limit: usize) -> Result<Vec<ITunesAlbum>> {
        let term = [artist, album].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" ");
        if term.is_empty() {
            return Ok(vec![]);
        }
        let u = with_query(
            "https://itunes.apple.com/search",
            &[
                ("term", &term),
                ("entity", "album"),
                ("media", "music"),
                ("country", country),
                ("limit", &limit.to_string()),
            ],
        );
        let r = get(&self.transport, &u, &self.user_agent, false)?;
        if r.status >= 400 {
            return Err(ProviderError::Http(r.status));
        }
        Self::parse(&r.body)
    }

    pub fn parse(data: &[u8]) -> Result<Vec<ITunesAlbum>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Root {
            results: Vec<Album>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Album {
            collection_id: Option<i64>,
            artist_name: Option<String>,
            collection_name: Option<String>,
            release_date: Option<String>,
            track_count: Option<i64>,
            country: Option<String>,
            artwork_url100: Option<String>,
        }
        let root: Root = serde_json::from_slice(data)?;
        Ok(root
            .results
            .into_iter()
            .filter_map(|r| {
                Some(ITunesAlbum {
                    collection_id: r.collection_id?,
                    artist: r.artist_name.unwrap_or_default(),
                    title: r.collection_name.unwrap_or_default(),
                    release_date: r.release_date.map(|d| crate::prefix(&d, 10)),
                    track_count: r.track_count,
                    country: r.country,
                    artwork_url_100: url(r.artwork_url100)?,
                })
            })
            .collect())
    }

    /// Artwork is served at any size by rewriting "100x100bb".
    pub fn resize(url: &str, size: u32) -> String {
        let re = regex::Regex::new(r"\d+x\d+bb").unwrap();
        re.replace_all(url, format!("{size}x{size}bb")).into_owned()
    }
}

// MARK: - fanart.tv

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FanartCover {
    pub url: String,
    pub likes: i64,
}

pub struct FanartClient {
    pub api_key: String,
    pub user_agent: String,
    transport: Arc<dyn Transport>,
}

impl FanartClient {
    pub fn new(api_key: &str, user_agent: &str) -> FanartClient {
        FanartClient { api_key: api_key.into(), user_agent: user_agent.into(), transport: default_transport() }
    }

    pub(crate) fn transport(&self) -> &Arc<dyn Transport> {
        &self.transport
    }

    /// Most liked first; empty when fanart.tv has nothing (404).
    pub fn album_covers(&self, release_group_id: &str) -> Result<Vec<FanartCover>> {
        let u = with_query(
            &format!("https://webservice.fanart.tv/v3/music/albums/{}", encode_path(release_group_id)),
            &[("api_key", &self.api_key)],
        );
        let r = get(&self.transport, &u, &self.user_agent, true)?;
        match r.status {
            404 => Ok(vec![]),
            s if s >= 400 => Err(ProviderError::Http(s)),
            _ => Ok(Self::parse(&r.body, release_group_id)),
        }
    }

    /// `{"albums": {"<rgid>": {"albumcover": [{"url": "…", "likes": "3"}]}}}`
    pub fn parse(data: &[u8], release_group_id: &str) -> Vec<FanartCover> {
        let Ok(root) = serde_json::from_slice::<serde_json::Value>(data) else { return vec![] };
        let Some(albums) = root.get("albums").and_then(|a| a.as_object()) else { return vec![] };
        let album = albums.get(release_group_id).or_else(|| albums.values().next());
        let mut covers: Vec<FanartCover> = album
            .and_then(|a| a.get("albumcover"))
            .and_then(|c| c.as_array())
            .into_iter()
            .flatten()
            .filter_map(|c| {
                let url = url(c.get("url")?.as_str().map(String::from))?;
                let likes = match c.get("likes") {
                    Some(serde_json::Value::String(s)) => s.parse().unwrap_or(0),
                    Some(serde_json::Value::Number(n)) => n.as_i64().unwrap_or(0),
                    _ => 0,
                };
                Some(FanartCover { url, likes })
            })
            .collect();
        covers.sort_by_key(|c| std::cmp::Reverse(c.likes));
        covers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cover_art_archive() {
        let json = r#"{"images":[{"id":123,"image":"https://archive.org/x/front.jpg","front":true,"back":false,"types":["Front"],"comment":"",
        "thumbnails":{"250":"https://a/250.jpg","500":"https://a/500.jpg","1200":"https://a/1200.jpg","small":"https://a/250.jpg","large":"https://a/500.jpg"}},
        {"id":124,"image":"https://archive.org/x/back.jpg","front":false,"back":true,"types":["Back","Spine"],"thumbnails":{}}]}"#;
        let images = CoverArtArchiveClient::parse(json.as_bytes()).unwrap();
        assert_eq!(images.len(), 2);
        assert!(images[0].is_front);
        assert_eq!(images[0].large.as_deref(), Some("https://a/1200.jpg"));
        assert_eq!(images[0].comment, None);
        assert!(images[1].is_back);
        assert_eq!(images[1].types, ["Back", "Spine"]);
        assert_eq!(images[1].large, None);
        assert_eq!(CoverArtArchiveClient::front_url("abc", 1200), "https://coverartarchive.org/release/abc/front-1200");
    }

    #[test]
    fn parses_itunes() {
        let json = r#"{"resultCount":1,"results":[{"collectionId":42,"artistName":"Miles Davis","collectionName":"Walkin'","releaseDate":"1957-01-01T08:00:00Z",
        "trackCount":5,"country":"USA","artworkUrl100":"https://is1-ssl.mzstatic.com/image/thumb/Music/v4/abc/source/100x100bb.jpg"}]}"#;
        let albums = ITunesSearchClient::parse(json.as_bytes()).unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].artist, "Miles Davis");
        assert_eq!(albums[0].release_date.as_deref(), Some("1957-01-01"));
        assert!(albums[0].artwork_url_3000().ends_with("/3000x3000bb.jpg"));
        assert!(albums[0].artwork_url_1500().ends_with("/1500x1500bb.jpg"));
    }

    #[test]
    fn parses_deezer() {
        let json = r#"{"data":[{"id":7,"title":"Walkin'","nb_tracks":5,"cover_xl":"https://e-cdns/1000x1000.jpg","cover_big":"https://e-cdns/500x500.jpg","artist":{"name":"Miles Davis"}}],"total":1}"#;
        let albums = DeezerClient::parse(json.as_bytes()).unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].id, 7);
        assert_eq!(albums[0].artist, "Miles Davis");
        assert_eq!(albums[0].cover_xl.as_deref(), Some("https://e-cdns/1000x1000.jpg"));
    }

    #[test]
    fn parses_fanart() {
        let json = br#"{"albums":{"rg":{"albumcover":[{"url":"https://f/a.jpg","likes":"1"},{"url":"https://f/b.jpg","likes":"5"},{"url":"https://f/c.jpg","likes":2}]}}}"#;
        let covers = FanartClient::parse(json, "rg");
        assert_eq!(covers.iter().map(|c| c.likes).collect::<Vec<_>>(), [5, 2, 1]);
    }
}
