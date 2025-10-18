use std::{
    env,
    error::Error,
    fmt::{self, Debug},
    path::PathBuf,
};

use regex::Regex;
use reqwest::redirect;
use rspotify::{
    model::{Country, FullTrack, Market, TrackId},
    prelude::{BaseClient, OAuthClient},
    AuthCodeSpotify,
};

pub async fn setup_spotify() -> AuthCodeSpotify {
    use rspotify::{scopes, AuthCodeSpotify, Credentials, OAuth};

    let creds =
        Credentials::from_env().expect("RSPOTIFY_CLIENT_ID or RSPOTIFY_CLIENT_SECRET not set");
    let oauth = OAuth {
        redirect_uri: "http://localhost:8888/callback".to_string(),
        scopes: scopes!(
            "user-read-private",
            "user-read-email",
            "user-read-playback-state",
            "user-modify-playback-state"
        ),
        ..Default::default()
    };
    let cache_path = env::var("RSPOTIFY_CACHE_PATH")
        .map(PathBuf::from)
        .unwrap_or(rspotify::Config::default().cache_path);
    let config = rspotify::Config {
        token_cached: true,
        token_refreshing: true,
        cache_path,
        ..rspotify::Config::default()
    };
    let mut spotify = AuthCodeSpotify::with_config(creds.clone(), oauth.clone(), config.clone());
    // attempt to read token cache from file and use token
    match spotify.read_token_cache(true).await {
        Ok(Some(token)) => {
            spotify = AuthCodeSpotify::from_token_with_config(token, creds, oauth, config);
            let token = spotify.get_token().lock().await.unwrap().clone();
            log::info!(
                "Using cached Spotify token, expires {}",
                token
                    .and_then(|t| t.expires_at.map(|d| d.to_string()))
                    .unwrap_or("unknown".to_string()),
            )
        }
        _ => log::info!("No Spotify token in cache"),
    }
    spotify
}

pub struct SpotifyTrackId {
    pub track_id: String,
}

impl SpotifyTrackId {
    pub fn from_id(id: String) -> Self {
        Self { track_id: id }
    }
    pub fn from_urn(urn: String) -> Option<Self> {
        let re = regex::Regex::new(r"(accept:)?spotify:track:(\w+)").unwrap();
        re.captures(&urn).and_then(|c| {
            c.get(2).map(|m| Self {
                track_id: m.as_str().into(),
            })
        })
    }
    pub async fn from_url(url: String) -> Option<Self> {
        let re_link = Regex::new(r"https?://spotify\.link/(\w+)").unwrap();
        let track_url = if re_link.is_match(&url) {
            Self::resolve_spotify_link(&url).await
        } else {
            None
        };

        let re_open = Regex::new(r"https?://open\.spotify\.com/track/(\w+)").unwrap();
        let open_url = &track_url.unwrap_or(url);
        let match_open_url = re_open.captures(open_url);
        match_open_url.and_then(|mat| {
            mat.get(1)
                .map(|m| m.as_str().to_string())
                .map(|id| Self { track_id: id })
        })
    }
    #[allow(dead_code)]
    pub fn track_urn(&self) -> String {
        format!("spotify:track:{}", self.track_id)
    }
    pub fn track_url(&self) -> String {
        format!("http://open.spotify.com/track/{}", self.track_id)
    }

    pub async fn resolve_spotify_link(url: &String) -> Option<String> {
        let custom = redirect::Policy::custom(|attempt| {
            if attempt.previous().len() > 5 {
                attempt.error("too many redirects")
            } else if attempt.url().host_str() == Some("spotify.com") {
                attempt.stop()
            } else {
                attempt.follow()
            }
        });
        let client = reqwest::Client::builder().redirect(custom).build().unwrap();
        let res = client.get(url).send().await.unwrap();
        Some(res.url().to_string())
    }
}

#[derive(Debug)]
pub enum FetchTrackError {
    InvalidTrackUrl(String),
    InvalidTrackUri(String),
    SpotifyApiError(rspotify::ClientError),
}

impl fmt::Display for FetchTrackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FetchTrackError::InvalidTrackUrl(url) => write!(f, "Invalid track URL: {}", url),
            FetchTrackError::InvalidTrackUri(uri) => write!(f, "Invalid track URI: {}", uri),
            FetchTrackError::SpotifyApiError(err) => write!(f, "Spotify API error: {}", err),
        }
    }
}

impl Error for FetchTrackError {}

impl From<rspotify::ClientError> for FetchTrackError {
    fn from(err: rspotify::ClientError) -> Self {
        FetchTrackError::SpotifyApiError(err)
    }
}

pub async fn fetch_track(
    spotify: &AuthCodeSpotify,
    track_url: String,
) -> Result<FullTrack, FetchTrackError> {
    let track_id = SpotifyTrackId::from_url(track_url.clone())
        .await
        .ok_or_else(|| FetchTrackError::InvalidTrackUrl(track_url.clone()))?;

    let track_urn = &track_id.track_urn();
    let track_id = TrackId::from_uri(track_urn)
        .map_err(|_| FetchTrackError::InvalidTrackUri(track_id.track_urn()))?;

    let track = spotify
        .track(track_id, Some(Market::Country(Country::Germany)))
        .await?;

    Ok(track)
}

pub fn format_track_text(track: &FullTrack) -> String {
    let artists = track
        .artists
        .iter()
        .map(|a| a.name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let duration = format!(
        "{}:{}",
        track.duration.num_minutes(),
        track.duration.num_seconds() % 60
    );
    let listen = track
        .external_urls
        .get("spotify")
        .map(|u| format!("<a href=\"{}\">Listen</a>", u));
    let covers = track
        .album
        .images
        .iter()
        .map(|i| format!("<a href=\"{}\">Cover</a>", i.url))
        .collect::<Vec<_>>()
        .join(" ");
    log::info!("track {:?}", track);
    format!(
        "🎵 <b>{}</b>\n👥 <b>{}</b>\n💿 <b>{}</b>\n🔥 {} • ⏱️ {}\n{} • {}",
        track.name,
        artists,
        track.album.name,
        track.popularity,
        duration,
        listen.unwrap_or("".to_string()),
        covers
    )
}
