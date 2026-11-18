//! Release identification (port of IdentifyKit): signals read off an
//! album, a candidate pool from MusicBrainz, Discogs and AcoustID, scoring
//! against the local tracks, cover art sources and a CDDB responder for
//! rippers. Image recognition (barcodes, OCR, cover comparison) arrives with
//! the vision phase through [`collector::ArtworkScanning`].

pub mod catalog;
pub mod cddb;
pub mod collector;
pub mod cover_art;
pub mod fingerprint;
pub mod identifier;
pub mod scoring;
pub mod signals;
pub mod text;

pub use catalog::{CatalogNumber, CatalogNumberParser};
pub use cddb::CDDBResponder;
pub use collector::{ArtworkScanning, CollectorOptions, SetMember, SignalCollector};
pub use cover_art::{ArtworkLocation, ArtworkOption, ArtworkSource, CoverArtFetcher, FetchedArtwork};
pub use fingerprint::{FingerprintOutcome, FingerprintService, ReleaseVote};
pub use identifier::{IdentificationResult, Identifier, IdentifierConfig, IdentifyProgress, ProviderClients};
pub use scoring::{CandidateOrigin, FormatCompatibility, MatchConfidence, MatchScorer, ScoredCandidate};
pub use signals::{AlbumSignals, ArtworkScanResult, LocalFormat, LocalTrack, SignalOrigin, SignalValue};
