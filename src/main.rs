#![recursion_limit = "512"]

use blased::{BlaseballClient, Player, Score, Team};
use cached::proc_macro::cached;
use edit_distance::edit_distance;
use futures::stream::{self, FuturesUnordered, TryStreamExt};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serenity::{
    async_trait,
    framework::standard::{macros::*, *},
    model::prelude::*,
    prelude::*,
};
use std::borrow::Cow;
use std::collections::HashSet;
use std::convert::TryInto;

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _: Context, ready: Ready) {
        println!("Connected as {}", ready.user.name);
    }

    async fn guild_create(&self, ctx: Context, guild: Guild, _is_new: bool) {
        let user = ctx.cache.current_user().await;
        let member = if let Ok(x) = guild.member(&ctx, user.id).await {
            x
        } else {
            println!("couldn't get own member in {}", guild.name);
            return;
        };
        if member.nick.is_none() {
            if let Err(err) = guild.edit_nickname(&ctx, Some("Oliver")).await {
                if !matches!(
                    err,
                    serenity::Error::Model(ModelError::InvalidPermissions(_))
                ) {
                    println!("couldn't set nickname in {}", guild.name);
                }
            } else {
                println!("set nickname in {}", guild.name);
            }
        }
    }
}

#[hook]
async fn after(_: &Context, _: &Message, cmd_name: &str, error: Result<(), CommandError>) {
    if let Err(why) = error {
        println!("Error in {}: {:?}", cmd_name, why);
    }
}

#[group]
#[commands(team, team_forbidden, player, player_forbidden)]
#[description = "Commands to look up information."]
struct Lookup;

#[cached(time = 300, result)]
async fn all_players(forbidden: bool) -> blased::Result<Vec<(Team, Player)>> {
    let client = BlaseballClient::new();
    let client = &client;
    let teams = client.all_teams().await?;
    let players: Vec<_> = teams
        .iter()
        .flat_map(|x| {
            x.lineup
                .iter()
                .chain(&x.rotation)
                .chain(if forbidden { &x.bullpen as &[_] } else { &[] })
                .chain(if forbidden { &x.bench as &[_] } else { &[] })
                .map(move |y| (x, &*y as &str))
        })
        .collect();
    players
        .chunks(50)
        .map(|chunk| async move {
            let chunk = chunk.into_iter().cloned().collect::<Vec<_>>();
            let players = client
                .players(&chunk.iter().map(|x| x.1).collect::<Vec<_>>())
                .await?;
            blased::Result::Ok(stream::iter(players.into_iter().map(move |player| {
                let team = chunk
                    .iter()
                    .find(|x| x.1 == player.id)
                    .expect("player missing")
                    .0
                    .clone();
                Ok((team, player))
            })))
        })
        .collect::<FuturesUnordered<_>>()
        .try_flatten()
        .try_collect::<Vec<_>>()
        .await
}

async fn team_imp(ctx: &Context, msg: &Message, args: Args, forbidden: bool) -> CommandResult {
    msg.channel_id.broadcast_typing(&ctx.http).await?;
    let client = BlaseballClient::new();
    let team_name = args.rest().to_ascii_lowercase();
    println!("searching for team {}", team_name);
    let teams = client.all_teams().await?;
    let team = teams
        .into_iter()
        .map(|x| {
            (
                edit_distance(&team_name, &x.nickname.to_ascii_lowercase())
                    .min(edit_distance(&team_name, &x.full_name.to_ascii_lowercase()))
                    .min(edit_distance(&team_name, &x.location.to_ascii_lowercase())),
                x,
            )
        })
        .min_by_key(|x| x.0)
        .filter(|x| x.0 < 3)
        .map(|x| x.1);
    if let Some(team) = team {
        println!("found team {}", team.full_name);

        let mut player_ids: Vec<&str> = team.lineup.iter().map(AsRef::as_ref).collect();
        player_ids.extend(team.rotation.iter().map(<String as AsRef<str>>::as_ref));
        if forbidden {
            player_ids.extend(team.bullpen.iter().map(<String as AsRef<str>>::as_ref));
            player_ids.extend(team.bench.iter().map(<String as AsRef<str>>::as_ref));
        }

        println!("searching for players");
        let mut players = client.players(&player_ids).await?.into_iter();
        println!("found players");

        msg.channel_id
            .send_message(&ctx.http, |m| {
                m.embed(|e| {
                    let color_str = &team.main_color[1..];
                    let color: u32 =
                        u32::from_str_radix(color_str, 16).expect("got invalid color from db");
                    let emoji_num_str = &team.emoji[2..];
                    let emoji_num: u32 =
                        u32::from_str_radix(emoji_num_str, 16).expect("got invalid emoji from db");
                    let emoji: char = emoji_num.try_into().expect("got invalid emoji from db");
                    let emoji_str = emoji.to_string();
                    let emoji_encoded = utf8_percent_encode(&emoji_str, NON_ALPHANUMERIC);
                    let png = format!("https://emojicdn.elk.sh/{}?style=twitter", emoji_encoded);
                    let url = format!("https://www.blaseball.com/team/{}", team.id);

                    let mut text = String::new();
                    text += "*";
                    text += &team.slogan;
                    text += "*\n\nLineup:\n";

                    for lineup_player in players.by_ref().take(9) {
                        text += "• ";
                        text += &lineup_player.name;
                        text += "\n";
                    }

                    text += "\nRotation:\n";

                    for rotation_player in players.by_ref().take(5) {
                        text += "• ";
                        text += &rotation_player.name;
                        text += "\n";
                    }

                    if forbidden {
                        text += "||\nUnused:\n";
                        for unused_player in players {
                            text += "• ";
                            text += &unused_player.name;
                            text += "\n";
                        }
                        text += "||";
                    }

                    e.title(team.full_name)
                        .color(color)
                        .thumbnail(png)
                        .url(url)
                        .description(text)
                        .fields(vec![
                            ("Total Shames", team.total_shames, true),
                            ("Total Shamings", team.total_shamings, true),
                            ("Season Shames", team.season_shames, true),
                            ("Season Shamings", team.season_shamings, true),
                            ("Championships", team.championships, true),
                        ])
                })
            })
            .await?;
        println!("sent embed");
    } else {
        println!("not found");
        msg.channel_id.say(&ctx.http, "Team not found.").await?;
    }
    Ok(())
}

#[command]
#[description("Search for a team.")]
#[usage("<team>")]
#[example("crabs")]
#[example("baltimore")]
async fn team(ctx: &Context, msg: &Message, args: Args) -> CommandResult {
    team_imp(ctx, msg, args, false).await
}

#[command]
#[description("Search for a team, including forbidden knowledge (||unused players||).")]
#[usage("<team>")]
#[example("crabs")]
#[example("baltimore")]
async fn team_forbidden(ctx: &Context, msg: &Message, args: Args) -> CommandResult {
    team_imp(ctx, msg, args, true).await
}

fn maybe_spoiler(s: &str, spoiler: bool) -> Cow<str> {
    if spoiler {
        format!("||{}||", s).into()
    } else {
        s.into()
    }
}

fn render_stars(mut stars: f64) -> String {
    if stars >= 0.5 {
        let mut s = String::new();
        loop {
            if stars >= 1.0 {
                s += "★";
                stars -= 1.0;
            } else {
                s += "⯨";
                break;
            }
        }
        s
    } else {
        "*None.*".to_string()
    }
}

async fn player_imp(ctx: &Context, msg: &Message, args: Args, forbidden: bool) -> CommandResult {
    msg.channel_id.broadcast_typing(&ctx.http).await?;
    let player_name = args.rest().to_ascii_lowercase();
    println!("searching for {}", player_name);
    let players = all_players(forbidden).await?;
    let (team, player) = players
        .into_iter()
        .min_by_key(|x| edit_distance(&player_name, &x.1.name.to_ascii_lowercase()))
        .expect("got no players");
    println!("found player {}", player.name);
    let player_unused = !team.lineup.contains(&player.id) && !team.rotation.contains(&player.id);
    msg.channel_id
        .send_message(&ctx.http, |m| {
            m.embed(|e| {
                let url = format!("https://www.blaseball.com/player/{}", player.id);
                let team_url = format!("https://www.blaseball.com/team/{}", team.id);
                let color_str = &team.main_color[1..];
                let color: u32 =
                    u32::from_str_radix(color_str, 16).expect("got invalid color from db");
                let emoji_num_str = &team.emoji[2..];
                let emoji_num: u32 =
                    u32::from_str_radix(emoji_num_str, 16).expect("got invalid emoji from db");
                let emoji: char = emoji_num.try_into().expect("got invalid emoji from db");
                let emoji_str = emoji.to_string();
                let emoji_encoded = utf8_percent_encode(&emoji_str, NON_ALPHANUMERIC);
                let png = format!("https://emojicdn.elk.sh/{}?style=twitter", emoji_encoded);

                e.title(maybe_spoiler(&player.name, player_unused)).url(url);

                if !player_unused {
                    e.color(color)
                        .author(|a| a.name(&team.full_name).url(team_url).icon_url(png));
                } else {
                    e.field("Team", format!("||{}||", team.full_name), true);
                }

                e.field("Batting", maybe_spoiler(&render_stars(player.rating(Score::Batting)), player_unused), true);
                e.field("Pitching", maybe_spoiler(&render_stars(player.rating(Score::Pitching)), player_unused), true);
                e.field("Baserunning", maybe_spoiler(&render_stars(player.rating(Score::Baserunning)), player_unused), true);
                e.field("Defense", maybe_spoiler(&render_stars(player.rating(Score::Defense)), player_unused), true);

                if let Some(bat) = player.bat.filter(|x| x != "") {
                    e.field("Bat", maybe_spoiler(&bat, player_unused), true);
                }
                if let Some(armor) = player.armor.filter(|x| x != "") {
                    e.field("Armor", maybe_spoiler(&armor, player_unused), true);
                }
                if let Some(ritual) = player.ritual.filter(|x| x != "") {
                    e.field(
                        "Pregame Ritual",
                        maybe_spoiler(&ritual, player_unused),
                        true,
                    );
                }
                if let Some(coffee) = player.coffee {
                    let styles = [
                        "Black",
                        "Light & Sweet",
                        "Macchiato",
                        "Cream & Sugar",
                        "Cold Brew",
                        "Flat White",
                        "Americano",
                        "Espresso",
                        "Heavy Foam",
                        "Latte",
                        "Decaf",
                        "Milk Substitute",
                        "Plenty of Sugar",
                        "Anything",
                    ];
                    e.field(
                        "Coffee Style",
                        maybe_spoiler(&styles[coffee], player_unused),
                        true,
                    );
                }
                if let Some(blood) = player.blood {
                    let types = [
                        "A", "AA", "AAA", "Acidic", "Basic", "O", "O No", "H₂O", "Electric",
                        "Love", "Fire", "Psychic", "Grass",
                    ];
                    e.field(
                        "Blood Type",
                        maybe_spoiler(&types[blood], player_unused),
                        true,
                    );
                }
                e.field(
                    "Fate",
                    maybe_spoiler(&player.fate.to_string(), player_unused),
                    true,
                );
                if forbidden {
                    macro_rules! make_name {
                        ($head:ident $(, $stat:ident)*) => {
                            concat!("||", stringify!($head), make_name!(, $($stat),*), "||")
                        };
                        (, $head:ident $(, $stat:ident)*) => {
                            concat!(", ", stringify!($head), make_name!(, $($stat),*))
                        };
                        (,) => {
                            ""
                        };
                    }
                    macro_rules! make_value {
                        ($head:ident $(, $stat:ident)*) => {
                            concat!("||{}", make_value!(, $($stat),*), "||")
                        };
                        (, $head:ident $(, $stat:ident)*) => {
                            concat!(", {}", make_value!(, $($stat),*))
                        };
                        (,) => {
                            ""
                        };
                    }
                    macro_rules! stat {
                        ($s1:ident, $s2:ident, $s3:ident, $s4:ident, $s5:ident, $s6:ident, $s7:ident, $($stats:ident),+) => {
                            stat!($s1, $s2, $s3, $s4, $s5, $s6, $s7);
                            stat!($($stats),+);
                        };
                        ($($stats:ident),+) => {
                            let name = make_name!($($stats),*);
                            let value = format!(make_value!($($stats),*), $(player.$stats),*);
                            e.field(name, value, true);
                        };
                        () => {};
                    }
                    stat!(anticapitalism, base_thirst, buoyancy, chasiness, coldness, continuation, divinity, ground_friction, indulgence, laserlikeness, martyrdom, moxie, musclitude, omniscience, overpowerment, patheticism, ruthlessness, shakespearianism, suppression, tenaciousness, thwackability, tragicness, unthwackability, watchfulness, pressurization, cinnamon);
                    e.field("||Fingers||", format!("||{}||", player.total_fingers), true);
                    e.field("Peanut Allergy", if player.peanut_allergy { "||`Yes`||" } else { "||`No `||" }, true);
                }

                e
            })
        })
        .await?;
    Ok(())
}

#[command]
#[description = "Search for a player."]
#[usage("<player>")]
#[example("Oliver Notarobot")]
async fn player(ctx: &Context, msg: &Message, args: Args) -> CommandResult {
    player_imp(ctx, msg, args, false).await
}

#[command]
#[description = "Search for a player, including forbidden knowledge (||unused players||, forbidden stats, and peanut allergy)."]
#[usage("<player>")]
#[example("Oliver Notarobot")]
async fn player_forbidden(ctx: &Context, msg: &Message, args: Args) -> CommandResult {
    player_imp(ctx, msg, args, true).await
}

#[help]
#[command_not_found_text = "Invalid command `{}`."]
#[individual_command_tip = "To get help with an individual command, pass its name as an argument to this command.\nForbidden knowledge is always spoilered."]
#[strikethrough_commands_tip_in_dm = ""]
#[strikethrough_commands_tip_in_guild = ""]
#[max_levenshtein_distance(3)]
async fn embed_help(
    context: &Context,
    msg: &Message,
    args: Args,
    help_options: &'static HelpOptions,
    groups: &[&'static CommandGroup],
    owners: HashSet<UserId>,
) -> CommandResult {
    let _ = help_commands::with_embeds(context, msg, args, help_options, groups, owners).await;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let token = dotenv::var("DISCORD_TOKEN")?;
    let framework = StandardFramework::new()
        .configure(|c| c.prefix("b!"))
        .after(after)
        .help(&EMBED_HELP)
        .group(&LOOKUP_GROUP);
    let mut client = Client::new(&token)
        .event_handler(Handler)
        .framework(framework)
        .await?;
    client.start().await?;
    Ok(())
}
