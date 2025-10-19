use log::warn;
use regex::Regex;
use rspotify::{
    model::TrackId,
    prelude::{BaseClient, OAuthClient},
};
use std::{env, error::Error, ops::Not};
use teloxide::{
    dispatching::{
        dialogue::{self, GetChatId, InMemStorage},
        UpdateHandler,
    },
    payloads::SendMessage,
    prelude::*,
    requests::JsonRequest,
    types::{
        InlineKeyboardButton, InlineKeyboardMarkup, LinkPreviewOptions, MaybeInaccessibleMessage,
        MessageId, ParseMode, ReplyParameters, ThreadId, User,
    },
    utils::command::BotCommands,
};

type MyDialogue = Dialogue<State, InMemStorage<State>>;
type HandlerResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

use tg_music_bot::spotify::*;

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

async fn request_track(
    bot: Bot,
    cfg: ConfigParameters,
    msg: Message,
    spotify: rspotify::AuthCodeSpotify,
) -> HandlerResult {
    let requester = format_author(msg.from.as_ref());

    match fetch_track(&spotify, msg.text().unwrap().into()).await {
        Ok(track) => {
            let id = track.id.as_ref().unwrap().to_string();
            let track_text = format_track_text(&track);
            let preview = link_preview_for_url(track.album.images.first().map(|i| i.url.clone()));
            // inform requester
            bot.send_message(
                msg.chat.id,
                format!("✅ Successfully requested track.\n\n{}", track_text),
            )
            .parse_mode(ParseMode::Html)
            .link_preview_options(preview.clone())
            .await?;

            // inform voting chat
            let buttons = vec![vec![
                InlineKeyboardButton::new(
                    "✅ Add to queue".to_string(),
                    teloxide::types::InlineKeyboardButtonKind::CallbackData(format!(
                        "accept:{}",
                        id
                    )),
                ),
                InlineKeyboardButton::new(
                    "❌ Decline".to_string(),
                    teloxide::types::InlineKeyboardButtonKind::CallbackData("decline".to_string()),
                ),
            ]];
            let keyboard = InlineKeyboardMarkup::new(buttons);

            let mut voting_msg = bot
                .send_message(
                    cfg.voting_chat,
                    format!("User {} requested:\n{}", requester, track_text),
                )
                .parse_mode(ParseMode::Html)
                .link_preview_options(preview)
                .reply_markup(keyboard);
            if let Some(thread) = cfg.voting_thread {
                voting_msg = voting_msg.message_thread_id(thread);
            }
            voting_msg.await?;
            Ok(())
        }
        Err(e) => {
            bot.send_message(msg.chat.id, "❌ Failed to find track.")
                .await?;
            HandlerResult::Err(format!("Failed to fetch a track {:?}", e).into())
        }
    }
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
                                "✅ {} has added to queue:\n{}",
                                format_author(Some(&q.from)),
                                msg.and_then(|m| m.text()).unwrap_or(&track.track_url())
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
            .parse_mode(ParseMode::Html)
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

fn msg_is_spotify_link(msg: Message) -> bool {
    msg.text().is_some_and(|text| {
        Regex::new(r"https?://(open\.spotify\.com|spotify\.link)/(\w+)")
            .unwrap()
            .find(text)
            .is_some()
    })
}

fn link_preview_for_url(url: Option<String>) -> LinkPreviewOptions {
    LinkPreviewOptions {
        is_disabled: false,
        url,
        prefer_small_media: false,
        prefer_large_media: false,
        show_above_text: true,
    }
}

#[derive(Clone, Default)]
pub enum State {
    #[default]
    Start,
    SpotifyLogin, // --> Start
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
