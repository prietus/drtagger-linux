//! Provider credentials in the system keyring (Secret Service on the host,
//! the Secret portal inside Flatpak). The app ships with no keys; the user
//! enters them in Preferences > Providers. Account names match the Mac app.

use anyhow::{Context, Result};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    AcoustId,
    Discogs,
    Fanart,
}

impl Provider {
    pub const ALL: [Provider; 3] = [Provider::AcoustId, Provider::Discogs, Provider::Fanart];

    /// Same account names as the Mac app's Keychain items.
    pub fn account(self) -> &'static str {
        match self {
            Provider::AcoustId => "acoustid.apikey",
            Provider::Discogs => "discogs.token",
            Provider::Fanart => "fanart.apikey",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Provider::AcoustId => "AcoustID API key",
            Provider::Discogs => "Discogs personal token",
            Provider::Fanart => "fanart.tv API key",
        }
    }

    pub fn from_name(name: &str) -> Option<Provider> {
        match name.to_ascii_lowercase().as_str() {
            "acoustid" => Some(Provider::AcoustId),
            "discogs" => Some(Provider::Discogs),
            "fanart" | "fanart.tv" => Some(Provider::Fanart),
            _ => None,
        }
    }

    fn attributes(self) -> HashMap<&'static str, &'static str> {
        HashMap::from([("application", crate::APP_ID), ("account", self.account())])
    }
}

/// Inside the Flatpak the keys go to a file keyring unlocked through the
/// Secret portal. Outside GNOME the portal may run without a Secret backend
/// (river, sway…): oo7 only falls back to the host's Secret Service when the
/// portal is missing altogether, so any failure of the sandboxed keyring
/// tries the host's (gnome-keyring, KWallet) too.
async fn keyring() -> Result<oo7::Keyring> {
    let keyring = match oo7::Keyring::new().await {
        Ok(k) => k,
        Err(sandboxed) if crate::trash::sandboxed() => {
            oo7::Keyring::host().await.with_context(|| format!("opening the keyring ({sandboxed})"))?
        }
        Err(e) => return Err(e).context("opening the keyring"),
    };
    keyring.unlock().await.context("unlocking the keyring")?;
    Ok(keyring)
}

/// The stored value, or `None` when the provider has no key.
pub async fn get(provider: Provider) -> Result<Option<String>> {
    let keyring = keyring().await?;
    let items = keyring.search_items(&provider.attributes()).await?;
    let Some(item) = items.first() else { return Ok(None) };
    let secret = item.secret().await?;
    let value = String::from_utf8_lossy(secret.as_bytes()).trim().to_string();
    Ok((!value.is_empty()).then_some(value))
}

/// Stores a trimmed value; an empty value deletes the item instead of
/// storing "".
pub async fn set(provider: Provider, value: &str) -> Result<()> {
    let keyring = keyring().await?;
    let value = value.trim();
    if value.is_empty() {
        keyring.delete(&provider.attributes()).await?;
    } else {
        let label = format!("drtagger: {}", provider.label());
        keyring.create_item(&label, &provider.attributes(), value, true).await.context("storing the key")?;
    }
    Ok(())
}
