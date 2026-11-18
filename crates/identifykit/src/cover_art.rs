//! Where a front cover can come from, best first: the release's own Cover
//! Art Archive image, its release group, fanart.tv, Discogs, iTunes,
//! Deezer, and the scans already in the folder.

use librarykit::DetectedAlbum;
use librarykit::path_serde;
use providerkit::http::{Request, Transport, default_transport};
use providerkit::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtworkSource {
    CoverArtArchive,
    CoverArtArchiveGroup,
    Fanart,
    Discogs,
    Itunes,
    Deezer,
    Local,
}

impl ArtworkSource {
    pub fn raw(self) -> &'static str {
        match self {
            ArtworkSource::CoverArtArchive => "coverArtArchive",
            ArtworkSource::CoverArtArchiveGroup => "coverArtArchiveGroup",
            ArtworkSource::Fanart => "fanart",
            ArtworkSource::Discogs => "discogs",
            ArtworkSource::Itunes => "itunes",
            ArtworkSource::Deezer => "deezer",
            ArtworkSource::Local => "local",
        }
    }
}

/// A remote image (URL) or a file in the album folder.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtworkLocation {
    Remote(String),
    Local(#[serde(with = "path_serde")] PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtworkOption {
    pub source: ArtworkSource,
    pub location: ArtworkLocation,
    /// "Cover Art Archive (release)", "iTunes · Artist – Title".
    pub label: String,
}

impl ArtworkOption {
    pub fn is_local(&self) -> bool {
        matches!(self.location, ArtworkLocation::Local(_))
    }

    pub fn id(&self) -> String {
        let loc = match &self.location {
            ArtworkLocation::Remote(u) => u.clone(),
            ArtworkLocation::Local(p) => format!("file://{}", p.display()),
        };
        format!("{}:{loc}", self.source.raw())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchedArtwork {
    pub option: ArtworkOption,
    pub data: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

pub struct CoverArtFetcher {
    pub user_agent: String,
    transport: Arc<dyn Transport>,
    caa: CoverArtArchiveClient,
    itunes: ITunesSearchClient,
    deezer: DeezerClient,
    fanart: Option<FanartClient>,
}

impl CoverArtFetcher {
    pub fn new(user_agent: &str, fanart_key: Option<&str>) -> CoverArtFetcher {
        CoverArtFetcher {
            user_agent: user_agent.into(),
            transport: default_transport(),
            caa: CoverArtArchiveClient::new(user_agent),
            itunes: ITunesSearchClient::new(user_agent),
            deezer: DeezerClient::new(user_agent),
            fanart: fanart_key.filter(|k| !k.is_empty()).map(|k| FanartClient::new(k, user_agent)),
        }
    }

    /// Every source that answers, in preference order. Only the listings
    /// are downloaded.
    pub fn options(
        &self,
        candidate: &Candidate,
        discogs: Option<&Candidate>,
        album: Option<&DetectedAlbum>,
        log: &dyn Fn(String),
    ) -> Vec<ArtworkOption> {
        let mut out = vec![];
        let remote = |source, url: String, label: String| ArtworkOption {
            source,
            location: ArtworkLocation::Remote(url),
            label,
        };
        if candidate.source == Source::Musicbrainz {
            let images = self.caa.images(&candidate.provider_id).ok();
            match images.as_ref().and_then(|i| i.iter().find(|x| x.is_front).or(i.first())) {
                Some(front) => out.push(remote(
                    ArtworkSource::CoverArtArchive,
                    front.full_url.clone(),
                    "Cover Art Archive (release)".into(),
                )),
                None => log("Cover Art Archive: no image for this release.".into()),
            }
            if let Some(group) = &candidate.release_group_id {
                out.push(remote(
                    ArtworkSource::CoverArtArchiveGroup,
                    format!("https://coverartarchive.org/release-group/{group}/front"),
                    "Cover Art Archive (release group)".into(),
                ));
            }
        }
        if let (Some(fanart), Some(group)) = (&self.fanart, &candidate.release_group_id) {
            match fanart.album_covers(group) {
                Ok(covers) => {
                    for c in covers.iter().take(2) {
                        out.push(remote(
                            ArtworkSource::Fanart,
                            c.url.clone(),
                            format!("fanart.tv · {} likes", c.likes),
                        ));
                    }
                    if covers.is_empty() {
                        log("fanart.tv: no cover for this release group.".into());
                    }
                }
                Err(_) => log("fanart.tv: request failed.".into()),
            }
        }
        let discogs = discogs.or(if candidate.source == Source::Discogs { Some(candidate) } else { None });
        if let Some(url) = discogs.and_then(|d| d.cover_art_url.clone()) {
            out.push(remote(ArtworkSource::Discogs, url, "Discogs".into()));
        }
        if let Ok(albums) = self.itunes.search_albums(&candidate.artist, &candidate.title, "us", 3)
            && let Some(hit) = albums.first()
        {
            out.push(remote(
                ArtworkSource::Itunes,
                hit.artwork_url_3000(),
                format!("iTunes · {} – {}", hit.artist, hit.title),
            ));
        }
        if let Ok(albums) = self.deezer.search_albums(&candidate.artist, &candidate.title, 3)
            && let Some(hit) = albums.first()
            && let Some(url) = &hit.cover_xl
        {
            out.push(remote(ArtworkSource::Deezer, url.clone(), format!("Deezer · {} – {}", hit.artist, hit.title)));
        }
        for file in Self::local_fronts(album) {
            let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            out.push(ArtworkOption {
                source: ArtworkSource::Local,
                location: ArtworkLocation::Local(file),
                label: format!("Folder · {name}"),
            });
        }
        log(format!("Artwork: {} source(s) available.", out.len()));
        out
    }

    /// Downloads (or reads) one option; None when it is not an image of
    /// at least 200 px.
    pub fn fetch(&self, option: &ArtworkOption) -> Option<FetchedArtwork> {
        let data = match &option.location {
            ArtworkLocation::Local(p) => std::fs::read(p).ok()?,
            ArtworkLocation::Remote(url) => {
                let r = self
                    .transport
                    .send(&Request::get(url.clone()).header("User-Agent", self.user_agent.clone()).timeout(30))
                    .ok()?;
                if r.status >= 400 || r.body.len() <= 1024 {
                    return None;
                }
                r.body
            }
        };
        let (width, height) = Self::dimensions(&data)?;
        if width < 200 {
            return None;
        }
        Some(FetchedArtwork { option: option.clone(), data, width, height })
    }

    /// The first option that yields a usable image.
    pub fn best(
        &self,
        candidate: &Candidate,
        discogs: Option<&Candidate>,
        album: Option<&DetectedAlbum>,
        log: &dyn Fn(String),
    ) -> Option<FetchedArtwork> {
        for option in self.options(candidate, discogs, album, log) {
            if let Some(art) = self.fetch(&option) {
                log(format!("Artwork: using {}, {}×{}.", option.label, art.width, art.height));
                return Some(art);
            }
        }
        None
    }

    pub fn local_fronts(album: Option<&DetectedAlbum>) -> Vec<PathBuf> {
        let Some(album) = album else { return vec![] };
        let rank = |u: &PathBuf| -> u8 {
            let n = u.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
            if n == "cover" || n == "front" || n == "folder" {
                0
            } else if n.contains("front") || n.contains("cover") || n.contains("frontal") {
                1
            } else if n.contains("back") || n.contains("obi") || n.contains("inlay") || n.contains("booklet") {
                9
            } else {
                5
            }
        };
        let mut files = album.artwork_files.clone();
        files.sort_by_key(rank);
        files.truncate(3);
        files
    }

    pub fn dimensions(data: &[u8]) -> Option<(usize, usize)> {
        imagesize::blob_size(data).ok().map(|s| (s.width, s.height))
    }

    pub fn local_dimensions(path: &Path) -> Option<(usize, usize)> {
        imagesize::size(path).ok().map(|s| (s.width, s.height))
    }
}
