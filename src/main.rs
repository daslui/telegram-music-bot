use log::warn;
use regex::Regex;
use reqwest::{self, redirect};
use rspotify::{
    model::TrackId,
    prelude::{BaseClient, OAuthClient},
    AuthCodeSpotify,
};
use std::{env, error::Error, ops::Not, path::PathBuf};
use teloxide::{
    dispatching::{
        dialogue::{self, GetChatId, InMemStorage},
        UpdateHandler,
    },
    payloads::SendMessage,
    prelude::*,
    requests::JsonRequest,
    types::{
        InlineKeyboardButton, InlineKeyboardMarkup, MaybeInaccessibleMessage, MessageId,
        ReplyParameters, ThreadId, User,
    },
    utils::command::BotCommands,
};

type MyDialogue = Dialogue<State, InMemStorage<State>>;
type HandlerResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::main]
async fn main() {
    pretty_env_logger::init();
    log::info!("Starting command bot...");

    let voting_chat = ChatId(
        env::var("TELEGRAM_VOTING_CHAT_ID")
            .expect("TELEGRAM_VOTING_CHAT_ID not set")
            .parse::<i64>()
            .expect("TELEGRAM_VOTING_THREAD_ID must be a number"),
    );
    let voting_thread = match env::var("TELEGRAM_VOTING_THREAD_ID") {
        Ok(thread) => thread.is_empty().not().then(|| {
            ThreadId(MessageId(
                thread
                    .parse::<i32>()
                    .expect("TELEGRAM_VOTING_THREAD_ID must be a number"),
            ))
        }),
        Err(_) => None,
    };
    let parameters = ConfigParameters {
        voting_chat,
        voting_thread,
    };

    let bot = Bot::from_env();

    Dispatcher::builder(bot, schema().await)
        .dependencies(dptree::deps![
            InMemStorage::<State>::new(),
            setup_spotify().await,
            parameters
        ])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
}

async fn setup_spotify() -> AuthCodeSpotify {
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
        .map(|v| PathBuf::from(v))
        .unwrap_or(rspotify::Config::default().cache_path);
    let config = rspotify::Config {
        token_cached: true,
        token_refreshing: true,
        cache_path: cache_path,
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

fn is_voting_chat(msg: Message, cfg: ConfigParameters) -> bool {
    msg.chat.id == cfg.voting_chat && msg.thread_id == cfg.voting_thread
}

async fn schema() -> UpdateHandler<Box<dyn std::error::Error + Send + Sync + 'static>> {
    use dptree::case;

    let admin_command_handler = teloxide::filter_command::<Command, _>().branch(
        case![State::Start]
            .branch(case![Command::Help].endpoint(help))
            .branch(case![Command::SpotifyLogin].endpoint(spotify_login)),
    );
    let command_handler = teloxide::filter_command::<Command, _>()
        .branch(case![Command::Help].endpoint(help))
        .branch(case![Command::Id].endpoint(id));

    let callback_handler = Update::filter_callback_query().branch(
        dptree::filter(|cfg: ConfigParameters, q: CallbackQuery| {
            q.chat_id()
                .is_some_and(|chat_id| chat_id == cfg.voting_chat)
        })
        .endpoint(handle_callback),
    );

    let message_handler = Update::filter_message()
        .branch(
            dptree::filter(|cfg: ConfigParameters, msg: Message| is_voting_chat(msg, cfg))
                .branch(admin_command_handler)
                .branch(case![State::SpotifyLogin].endpoint(spotify_login_token)),
        )
        .branch(command_handler)
        .branch(
            dptree::filter(|cfg: ConfigParameters, msg: Message| {
                msg.chat.is_private() && !is_voting_chat(msg, cfg)
            })
            .branch(dptree::filter(|msg: Message| msg_is_spotify_link(msg)).endpoint(request_track))
            .endpoint(user_help),
        );

    dialogue::enter::<Update, InMemStorage<State>, State, _>()
        .branch(callback_handler)
        .branch(message_handler)
}

fn msg_is_spotify_link(msg: Message) -> bool {
    msg.text().is_some_and(|text| {
        Regex::new(r"https?://(open\.spotify\.com|spotify\.link)/(\w+)")
            .unwrap()
            .find(text)
            .is_some()
    })
}

async fn spotify_login(
    bot: Bot,
    dialogue: MyDialogue,
    spotify: rspotify::AuthCodeSpotify,
    msg: Message,
) -> HandlerResult {
    match spotify.get_authorize_url(false) {
        Ok(auth_url) => {
            let send_msg = bot.send_message(msg.chat.id, format!("Spotify Login\nOpen this URL in the browser and allow Spotify access: {}\nThen paste and send the redirected URL here.", auth_url));
            set_reply(msg, send_msg).await?;
            dialogue.update(State::SpotifyLogin).await?;
        }
        Err(e) => {
            let send_msg: JsonRequest<SendMessage> =
                bot.send_message(msg.chat.id, format!("Spotify link error: {}", e));
            set_reply(msg, send_msg).await?;
            dialogue.update(State::Start).await?;
        }
    }
    Ok(())
}

async fn spotify_login_token(
    bot: Bot,
    dialogue: MyDialogue,
    spotify: rspotify::AuthCodeSpotify,
    msg: Message,
) -> HandlerResult {
    match msg
        .text()
        .and_then(|text| spotify.parse_response_code(text))
    {
        Some(code) => {
            spotify.request_token(&code).await?;
            spotify.write_token_cache().await?;
            let send_msg: JsonRequest<SendMessage> = bot.send_message(msg.chat.id, "Token saved");
            set_reply(msg, send_msg).await?;
        }
        _ => {
            let send_msg: JsonRequest<SendMessage> =
                bot.send_message(msg.chat.id, "Invalid Code/URL");
            set_reply(msg, send_msg).await?;
        }
    }
    dialogue.update(State::Start).await?;
    Ok(())
}

async fn request_track(bot: Bot, cfg: ConfigParameters, msg: Message) -> HandlerResult {
    let requester = format_author(msg.from.as_ref());
    let track = SpotifyTrackId::from_url(msg.text().unwrap().into());
    match track.await {
        Some(track) => {
            // inform requester
            bot.send_message(
                msg.chat.id,
                format!("Track wurde angefragt: {}", track.track_url()),
            )
            .await?;

            // inform voting chat
            let buttons = vec![vec![
                InlineKeyboardButton::new(
                    "✅ In Queue".to_string(),
                    teloxide::types::InlineKeyboardButtonKind::CallbackData(format!(
                        "accept:spotify:track:{}",
                        track.track_id
                    )),
                ),
                InlineKeyboardButton::new(
                    "❌ Löschen".to_string(),
                    teloxide::types::InlineKeyboardButtonKind::CallbackData("decline".to_string()),
                ),
            ]];
            let keyboard = InlineKeyboardMarkup::new(buttons);

            let mut voting_msg = bot
                .send_message(
                    cfg.voting_chat,
                    format!("Anfrage von {}:\n{}", requester, track.track_url()),
                )
                .reply_markup(keyboard);
            if let Some(thread) = cfg.voting_thread {
                voting_msg = voting_msg.message_thread_id(thread);
            }
            voting_msg.await?;
        }
        None => {
            bot.send_message(msg.chat.id, "Failed to request track")
                .await?;
        }
    }
    Ok(())
}

async fn handle_callback(
    bot: Bot,
    q: CallbackQuery,
    spotify: rspotify::AuthCodeSpotify,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let msg = match q.message {
        Some(MaybeInaccessibleMessage::Regular(ref message)) => Some(message),
        _ => None,
    };
    let mut disable_preview = false;
    let reply = match q.data.as_deref() {
        Some(accept) if accept.starts_with("accept:") => {
            match SpotifyTrackId::from_urn(accept.into()) {
                Some(track) => match TrackId::from_id(&track.track_id) {
                    Ok(trackid) => match spotify.add_item_to_queue(trackid.into(), None).await {
                        Ok(_) => {
                            format!(
                                "✅ {} hat akzeptiert: {}",
                                format_author(Some(&q.from)),
                                track.track_url()
                            )
                        }
                        Err(err) => {
                            warn!("Failed to queue track {err}");
                            format!("Failed to queue track: {}", err)
                        }
                    },
                    Err(_) => "Invalid track ID".into(),
                },
                None => "Invalid track ID".into(),
            }
        }
        Some("decline") => {
            disable_preview = true;
            let author = format_author(Some(&q.from));
            match msg.and_then(|msg| msg.text()) {
                Some(text) => format!("❌ {} hat abgelehnt: {}", author, text),
                None => format!("❌ {} hat abgelehnt.", author),
            }
        }
        _ => return Ok(()),
    };
    // edit existing message with status or send a new message
    if let Some(msg) = msg {
        let mut update = bot
            .edit_message_text(msg.chat.id, msg.id, reply)
            .reply_markup(InlineKeyboardMarkup::default());
        if disable_preview {
            update = update.link_preview_options(teloxide::types::LinkPreviewOptions {
                is_disabled: true,
                url: None,
                prefer_small_media: false,
                prefer_large_media: false,
                show_above_text: false,
            });
        }
        update.await?;
    } else if let Some(chat_id) = q.chat_id() {
        bot.send_message(chat_id, reply).await?;
    }
    Ok(())
}

fn format_author(from: Option<&User>) -> String {
    if let Some(from) = from {
        match &from.username {
            Some(username) => format!("{} (@{})", from.first_name, username),
            None => from.first_name.to_string(),
        }
    } else {
        "Unknown".into()
    }
}

async fn id(bot: Bot, msg: Message) -> HandlerResult {
    let answer = match msg.thread_id {
        Some(thread) => format!("This chat has ID {}, thread {}", msg.chat.id, thread),
        None => format!("This chat has ID {}", msg.chat.id),
    };
    let send_msg = bot.send_message(msg.chat.id, answer);
    set_reply(msg, send_msg).await?;
    Ok(())
}

fn set_reply(msg: Message, send_msg: JsonRequest<SendMessage>) -> JsonRequest<SendMessage> {
    if let Some(thread) = msg.thread_id {
        send_msg.reply_parameters(ReplyParameters::new(thread.0))
    } else {
        send_msg
    }
}

async fn help(bot: Bot, msg: Message) -> HandlerResult {
    bot.send_message(msg.chat.id, Command::descriptions().to_string())
        .await?;
    Ok(())
}

async fn user_help(bot: Bot, msg: Message) -> HandlerResult {
    bot.send_message(msg.chat.id, "Send a Spotify URL.").await?;
    Ok(())
}

struct SpotifyTrackId {
    pub track_id: String,
}

impl SpotifyTrackId {
    #[allow(dead_code)]
    fn from_id(id: String) -> Self {
        Self { track_id: id }
    }
    fn from_urn(urn: String) -> Option<Self> {
        let re = regex::Regex::new(r"(accept:)?spotify:track:(\w+)").unwrap();
        re.captures(&urn).and_then(|c| {
            c.get(2).map(|m| Self {
                track_id: m.as_str().into(),
            })
        })
    }
    async fn from_url(url: String) -> Option<Self> {
        let re_link = Regex::new(r"https?://spotify\.link/(\w+)").unwrap();
        let track_url = if re_link.is_match(&url) {
            Self::resolve_spotify_link(&url).await
        } else {
            None
        };

        let re_open = Regex::new(r"https?://open\.spotify\.com/track/(\w+)").unwrap();
        let open_url = &track_url.unwrap_or(url);
        let match_open_url = re_open.captures(&open_url);
        match_open_url.and_then(|mat| {
            mat.get(1)
                .map(|m| m.as_str().to_string())
                .map(|id| Self { track_id: id })
        })
    }
    #[allow(dead_code)]
    fn track_urn(&self) -> String {
        format!("spotify:track:{}", self.track_id)
    }
    fn track_url(&self) -> String {
        format!("http://open.spotify.com/track/{}", self.track_id)
    }

    async fn resolve_spotify_link(url: &String) -> Option<String> {
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

#[derive(Clone, Default)]
pub enum State {
    #[default]
    Start,
    SpotifyLogin, // --> Start
    ReceiveFullName,
    ReceiveProductChoice {
        full_name: String,
    },
}

#[derive(Clone)]
struct ConfigParameters {
    voting_chat: ChatId,
    voting_thread: Option<ThreadId>,
}

#[derive(BotCommands, Clone)]
#[command(
    rename_rule = "lowercase",
    description = "These commands are supported:"
)]
enum Command {
    #[command(description = "display this text.")]
    Help,
    #[command(description = "link spotify (admin only)")]
    SpotifyLogin,
    #[command(description = "get chat/thread id")]
    Id,
}
