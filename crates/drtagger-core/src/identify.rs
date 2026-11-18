//! Release identification and the CUETools DB check for queued albums
//! (the Mac app's IdentifyService and DiscService.checkCUEToolsDB).

use crate::keys::{self, Provider};
use crate::store::{AlbumRecord, AlbumState, Store, now_ms};
use anyhow::Result;
use identifykit::{IdentificationResult, Identifier, IdentifierConfig, IdentifyProgress, MatchConfidence, SetMember};
use providerkit::{CUEToolsDBClient, CtdbMetadata, CtdbVerification};
use serde::{Deserialize, Serialize};
use splitkit::FFmpegTool;
use std::path::PathBuf;
use std::sync::Arc;

pub const USER_AGENT: &str = concat!("drtagger/", env!("CARGO_PKG_VERSION"), " (+https://drtagger.priet.us)");

/// What CUETools DB knows about a disc's TOC, with the verdict on the
/// split tracks' CRCs when there are some.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CtdbReport {
    /// Milliseconds since the Unix epoch.
    pub checked_at: i64,
    pub entry_count: usize,
    pub total_confidence: i64,
    pub best_confidence: i64,
    pub metadata: Vec<CtdbMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<CtdbVerification>,
}

impl CtdbReport {
    pub fn is_known_disc(&self) -> bool {
        self.entry_count > 0
    }
}

/// The provider keys from the keyring; a missing one disables its service.
#[derive(Clone, Debug, Default)]
pub struct ProviderKeys {
    pub acoust_id: Option<String>,
    pub discogs: Option<String>,
    pub fanart: Option<String>,
}

impl ProviderKeys {
    /// From the keyring; scripts can pass them in ACOUSTID_KEY,
    /// DISCOGS_TOKEN and FANART_KEY instead (the Mac app's --acoustid-key
    /// launch arguments), so no keyring is needed.
    pub fn load() -> ProviderKeys {
        let get = |p| {
            let var = match p {
                Provider::AcoustId => "ACOUSTID_KEY",
                Provider::Discogs => "DISCOGS_TOKEN",
                Provider::Fanart => "FANART_KEY",
            };
            std::env::var(var)
                .ok()
                .filter(|k| !k.trim().is_empty())
                .or_else(|| crate::runtime().block_on(keys::get(p)).ok().flatten())
        };
        ProviderKeys {
            acoust_id: get(Provider::AcoustId),
            discogs: get(Provider::Discogs),
            fanart: get(Provider::Fanart),
        }
    }
}

/// Asks CUETools DB about the album's TOC (computing it first when
/// needed) and checks the split tracks' CRCs against every known rip.
pub fn check_ctdb(store: &Store, record: &mut AlbumRecord, tool: &FFmpegTool) -> Result<()> {
    if record.toc().is_none() {
        let toc = crate::disc::compute_toc(record, tool)?;
        record.set_toc(toc.as_ref());
        store.save(record)?;
    }
    let Some(toc) = record.toc() else { return Ok(()) };
    let response = CUEToolsDBClient::new(USER_AGENT).lookup(&toc.ctdb_toc_string())?;
    let verification = record
        .split_outcomes()
        .first()
        .and_then(|o| o.ctdb_track_crc32s().map(|crcs| CUEToolsDBClient::verify(&crcs, o.ctdb_disc_crc32, &response)));
    record.set_ctdb(Some(&CtdbReport {
        checked_at: now_ms(),
        entry_count: response.entries.len(),
        total_confidence: response.total_confidence(),
        best_confidence: response.best_entry().map_or(0, |e| e.confidence),
        metadata: response.metadata.clone(),
        verification,
    }));
    store.save(record)?;
    Ok(())
}

/// DSFs extracted from a SACD image's stereo area, in track order.
pub fn extracted_stereo_tracks(record: &AlbumRecord) -> Vec<PathBuf> {
    if record.kind != librarykit::AlbumKind::SacdIso {
        return vec![];
    }
    let outcomes = record.sacd_outcomes();
    let Some(stereo) = outcomes.iter().find(|o| !o.is_multichannel) else { return vec![] };
    let mut tracks = stereo.tracks.clone();
    tracks.sort_by_key(|t| t.number);
    tracks.into_iter().map(|t| t.url).collect()
}

/// Identifies one album, or a set when `members` has several records (the
/// discs of one release, each with its `set_position`). The result, the
/// state and the selected candidate are stored on every member.
pub fn identify(
    store: &Store,
    members: &mut [AlbumRecord],
    positions: &[i64],
    tool: &FFmpegTool,
    keys: &ProviderKeys,
    fast_path_when_tagged: bool,
    progress: &(dyn Fn(IdentifyProgress) + Sync),
) -> Result<IdentificationResult> {
    for m in members.iter_mut() {
        m.state = AlbumState::Identifying;
        store.save(m)?;
    }
    let mut config = IdentifierConfig::new(USER_AGENT);
    config.acoust_id_key = keys.acoust_id.clone();
    config.discogs_token = keys.discogs.clone();
    config.fast_path_when_tagged = fast_path_when_tagged;
    config.fast_path_with_barcode = fast_path_when_tagged;
    config.artwork_scanner = Some(Arc::new(visionkit::ArtworkScanner::default()));
    let identifier = Identifier::new(config, tool.clone());
    let ctdb: Vec<CtdbMetadata> = members[0].ctdb().map(|r| r.metadata).unwrap_or_default();
    let result = if members.len() > 1 {
        let set: Vec<SetMember> = members
            .iter()
            .zip(positions.iter().copied().chain(std::iter::repeat(1)))
            .filter_map(|(m, position)| {
                Some(SetMember {
                    album: m.detected()?,
                    position,
                    toc: m.toc(),
                    extracted_tracks: extracted_stereo_tracks(m),
                })
            })
            .collect();
        // Only what the discs themselves declare ("disc 2 of 3", SACD album set).
        let declared = members
            .iter()
            .filter_map(|m| m.detected().and_then(|d| d.disc_position()).and_then(|p| p.1))
            .max()
            .map(|t| t as i64);
        identifier.identify_set(&set, declared, &ctdb, progress)
    } else {
        let Some(album) = members[0].detected() else { anyhow::bail!("The album has no scan details; rescan it.") };
        let toc = members[0].toc();
        identifier.identify(&album, toc.as_ref(), &ctdb, &extracted_stereo_tracks(&members[0]), progress)
    };
    let (selected, state) = match result.best() {
        Some(best) if result.is_confident() => (Some(best.id()), AlbumState::Confident),
        best => (
            best.filter(|b| b.confidence >= MatchConfidence::Likely).map(|b| b.id()),
            if result.candidates.is_empty() { AlbumState::Scanned } else { AlbumState::NeedsReview },
        ),
    };
    for m in members.iter_mut() {
        m.set_identification(Some(&result));
        m.selected_candidate_id = selected.clone();
        m.state = state;
        store.save(m)?;
    }
    Ok(result)
}
