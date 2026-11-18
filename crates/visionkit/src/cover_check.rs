//! Does the cover the album already has look like the chosen release's?
//! (IdentifyKit's CoverCheck.) Measured with Vision on a real library:
//! the same artwork downloaded twice ≈ 0, home scans of it 0.06–0.8, other
//! covers of one series ≈ 0.8, unrelated covers 0.95 and up. Scans and
//! series overlap, so only an unmistakable difference is reported. The
//! DINOv2 distances are mapped onto that scale.

use crate::embedding::{FeaturePrint, FeaturePrinter};
use identifykit::ProviderClients;
use providerkit::http::{Request, Transport, default_transport};
use providerkit::{Candidate, CoverArtArchiveClient, Source};
use std::sync::Arc;

pub struct CoverCheck {
    user_agent: String,
    discogs_token: Option<String>,
    transport: Arc<dyn Transport>,
}

impl CoverCheck {
    pub const DIFFERENT_THRESHOLD: f32 = 0.95;

    pub fn new(user_agent: &str, discogs_token: Option<&str>) -> CoverCheck {
        CoverCheck {
            user_agent: user_agent.into(),
            discogs_token: discogs_token.map(Into::into),
            transport: default_transport(),
        }
    }

    /// The smallest distance between `local` and the release's front
    /// images; None when either side has no usable image.
    pub fn distance(&self, local: &[u8], candidate: &Candidate) -> Option<f32> {
        let printer = FeaturePrinter::shared()?;
        let mine = printer.feature_print_data(local)?;
        let mut best: Option<f32> = None;
        for url in self.front_images(candidate) {
            let Some(theirs) = self.download(&url).and_then(|d| printer.feature_print_data(&d)) else { continue };
            let d = mine.distance(&theirs);
            best = Some(best.map_or(d, |b| b.min(d)));
            if d < 0.3 {
                break;
            }
        }
        best
    }

    fn front_images(&self, c: &Candidate) -> Vec<String> {
        match c.source {
            Source::Musicbrainz => CoverArtArchiveClient::new(&self.user_agent)
                .images(&c.provider_id)
                .unwrap_or_default()
                .into_iter()
                .filter(|i| i.is_front)
                .take(3)
                .map(|i| i.small.or(i.thumbnail).unwrap_or(i.full_url))
                .collect(),
            Source::Discogs => {
                let clients = ProviderClients::shared(&self.user_agent, None, self.discogs_token.as_deref());
                let Some(discogs) = clients.discogs.as_ref() else { return vec![] };
                discogs
                    .images(&c.provider_id)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|i| i.is_primary)
                    .take(1)
                    .map(|i| i.thumbnail.unwrap_or(i.url))
                    .collect()
            }
            _ => vec![],
        }
    }

    fn download(&self, url: &str) -> Option<Vec<u8>> {
        let response =
            self.transport.send(&Request::get(url).header("User-Agent", self.user_agent.clone()).timeout(30)).ok()?;
        (response.status == 200).then_some(response.body)
    }

    /// Distance between two encoded images.
    pub fn distance_between(a: &[u8], b: &[u8]) -> Option<f32> {
        let printer = FeaturePrinter::shared()?;
        let (pa, pb): (FeaturePrint, FeaturePrint) = (printer.feature_print_data(a)?, printer.feature_print_data(b)?);
        Some(pa.distance(&pb))
    }
}
