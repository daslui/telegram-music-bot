import { Telegraf } from 'telegraf';
import storage from 'node-persist';
import SpotifyWebApi from 'spotify-web-api-node';
import rateLimit from 'telegraf-ratelimit';

// Telegram settings
const votingGroup = "-1002454626909";
const votingGroupThread = "637";
const admins = ["117441870"];
const deleteMessageAfterAcceptDecline = false;

// Spotify globals
var spotifyApi;
var spotifyRefreshToken;
var spotifyAccessToken;
var spotifyTokenExpires = Date.now();

function isAdmin(id) {
  return admins.includes(String(id));
}

function isVotingGroup(id) {
  return String(id) == String(votingGroup)
}

function getSpotify(refresh = true) {
  if (spotifyApi !== undefined && refresh === true) {
    refreshSpotifyToken(spotifyRefreshToken, spotifyAccessToken)
  }
  return spotifyApi;
}

function refreshSpotifyToken(refreshToken, accessToken, force = false) {
  if (new Date() >= spotifyTokenExpires || force === true) {
    spotifyApi.setRefreshToken(refreshToken)
    spotifyApi.setAccessToken(accessToken)
    spotifyApi.refreshAccessToken().then(
      function (data) {
        console.log('The access token has been refreshed!');

        // Save the access token so that it's used in future calls
        spotifyApi.setAccessToken(data.body['access_token']);
        spotifyTokenExpires = Date.now() + data.body['expires_in'] * 1000;
        console.debug("New expiry: " + new Date(spotifyTokenExpires).toISOString())
      },
      function (err) {
        console.log('Could not refresh access token', err);
      }
    );
  }
}

function spotifyUrlToUri(url) {
  if (!url) {
    return undefined
  }
  const regex = /https?:\/\/open\.spotify\.com\/track\/(\w+)/;
  let matches = url.match(regex);
  if (matches) {
    return "spotify:track:" + matches[1]
  }
  return undefined
}

async function resolveTrack(uri) {
  let matches = uri.match(/spotify:track:(\w+)/)
  if (!matches) {
    return undefined
  } else {
    let trackId = matches[1]
    let track = await getSpotify().getTrack(trackId)
    return track
  }
}

function addToSpotifyQueue(uri) {
  return getSpotify().addToQueue(uri)
}

function formatTrackInfo(trackInfo) {
  return {
    name: trackInfo.body.name,
    artists: trackInfo.body.artists.map(obj => obj.name).join(', '),
  }
}

function formatUser(user) {
  return (user.username) ? `${user.first_name} (@${user.username})` : `${user.first_name}`;
}

// Setup storage
storage.initSync( /* options ... */);

// Setup Spotify
if (process.env.SPOTIFY_CLIENT_ID === undefined
  || process.env.SPOTIFY_CLIENT_SECRET === undefined) {
  throw new TypeError("SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET must be provided!");
}
spotifyApi = new SpotifyWebApi({
  clientId: process.env.SPOTIFY_CLIENT_ID,
  clientSecret: process.env.SPOTIFY_CLIENT_SECRET,
  redirectUri: 'https://luisental.org/noop'
});
spotifyRefreshToken = await storage.get("spotify_refresh_token")
spotifyAccessToken = await storage.get("spotify_access_token")
if (spotifyRefreshToken !== undefined && spotifyAccessToken !== undefined) {
  refreshSpotifyToken(spotifyRefreshToken, spotifyAccessToken);
}

// Setup Bot
if (process.env.BOT_TOKEN === undefined) {
  throw new TypeError("BOT_TOKEN must be provided!");
}
const bot = new Telegraf(process.env.BOT_TOKEN)
// const store = SQLite({
// 	filename: "./telegraf-sessions.sqlite",
// });
// bot.use(session({ store }));
const trackLimitConfig = {
  window: 5 * 60 * 1000,
  limit: 3,
  keyGenerator: function (ctx) {
    return ctx.from.id
  },
  onLimitExceeded: (ctx) => ctx.reply('Limit überschritten, bitte warten!')
}
bot.use(rateLimit())
const helpText = `Sende eine Spotify-URL, um einen Musikwunsch zu stellen.
Dies geht direkt in der Spotify-App über *Teilen* > *Link teilen*.\n
Beispiel: https://open.spotify.com/track/5hvIZF56tE8sAwMA9cKmQQ?si=8e4ab90fe2654448`
bot.start((ctx) => ctx.reply(helpText))
bot.help((ctx) => ctx.reply(helpText))
bot.command('spotifylogin', (ctx) => {
  if (isAdmin(ctx.from.id)) {
    var scopes = [
      'user-read-private', // ??
      'user-read-email', // ??
      'user-read-playback-state', // not needed
      'user-modify-playback-state' // add to queue
    ]
    var authorizeURL = spotifyApi.createAuthorizeURL(scopes, 'not-random-state');
    ctx.reply("Error: SPOTIFY_CODE not set. Visit " + authorizeURL);
  }
})
bot.command('spotifytoken', (ctx) => {
  if (isAdmin(ctx.from.id)) {
    let spotifyCode = ctx.args[0]
    spotifyApi.authorizationCodeGrant(spotifyCode).then(
      function (data) {
        ctx.reply('Authorized. The token expires in ' + data.body['expires_in']);
        console.log('The token expires in ' + data.body['expires_in']);
        console.log('The access token is ' + data.body['access_token']);
        console.log('The refresh token is ' + data.body['refresh_token']);

        // Set the access token on the API object to use it in later calls
        spotifyApi.setAccessToken(data.body['access_token']);
        spotifyApi.setRefreshToken(data.body['refresh_token']);
        storage.setItem('spotify_refresh_token', data.body['refresh_token']);
        storage.setItem('spotify_access_token', data.body['access_token']);
        ctx.reply("Login finished and saved");
      },
      function (err) {
        console.log('Something went wrong!', err);
        ctx.reply('Something went wrong!', err);
      }
    );
  }
})
bot.action(/^accept:(spotify:track:\w+)/, (ctx) => {
  var uri = ctx.match[1]  // can not fail ;)
  addToSpotifyQueue(uri).then(async () => {
    let track = await resolveTrack(uri)
    let trackDescription = uri
    if (track) {
      let trackInfo = formatTrackInfo(track)
      trackDescription = `${trackInfo.name} • ${trackInfo.artists}`
    }
    ctx.reply(`Lied zur Queue hinzugefügt: ${trackDescription}`);
    if (deleteMessageAfterAcceptDecline) {
      ctx.deleteMessage()
    } else {
      ctx.editMessageText(`Von ${formatUser(ctx.update.callback_query.from)} abgelehnt: ${ctx.text}`);
      ctx.editMessageReplyMarkup(undefined);
    }
  })
})
bot.action('decline', (ctx) => {
  if (deleteMessageAfterAcceptDecline) {
    ctx.deleteMessage()
  } else {
    ctx.editMessageText(`Von ${formatUser(ctx.update.callback_query.from)} abgelehnt: ${ctx.text}`);
    ctx.editMessageReplyMarkup(undefined);
  }
})
bot.command("id", (ctx) => {
  if (isAdmin(ctx.from.id)) {
    ctx.reply(ctx.chat.id)
  }
})
bot.on('message', rateLimit(trackLimitConfig), async (ctx) => {
  if (isVotingGroup(ctx.chat.id)) {
    return;
  }
  var spotifyUri = spotifyUrlToUri(ctx.message.text)
  if (spotifyUri === undefined) {
    ctx.reply("Keine gültige Spotify-URL. Sende /help für Hilfe.")
  } else {
    let trackInfo = await resolveTrack(spotifyUri);
    let trackDescription = formatTrackInfo(trackInfo);
    let requester = formatUser(ctx.update.message.from);

    bot.telegram.sendMessage(votingGroup, "Anfrage von " + requester + "\n"
      + `${trackDescription.name} • ${trackDescription.artists}`, {
      reply_markup: {
        inline_keyboard: [
          /* Inline buttons. 2 side-by-side */
          [{ text: "✅ In Queue", callback_data: "accept:" + spotifyUri },
          { text: "❌ Löschen", callback_data: "decline" }],

          /* Also, we can have URL buttons. */
          [{ text: "Auf Spotify.com anzeigen", url: trackInfo.body.external_urls.spotify }]
        ]
      },
      ...(votingGroupThread && { 'message_thread_id': votingGroupThread })
    });
    ctx.reply(`${trackDescription.name} von ${trackDescription.artists} wurde angefragt.`)
  }
})
bot.launch()

// Enable graceful stop
process.once('SIGINT', () => bot.stop('SIGINT'))
process.once('SIGTERM', () => bot.stop('SIGTERM'))
