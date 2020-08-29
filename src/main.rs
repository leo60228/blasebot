use cached::proc_macro::cached;
use edit_distance::edit_distance;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serenity::{
    async_trait,
    framework::standard::{macros::*, *},
    model::prelude::*,
    prelude::*,
};
use std::collections::HashSet;
use std::convert::TryInto;

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _: Context, ready: Ready) {
        println!("Connected as {}", ready.user.name);
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
struct Lookup;

#[cached(time = 300, result)]
async fn all_players() -> blased::Result<Vec<(blased::Team, blased::Player)>> {
    let client = blased::BlaseballClient::new();
    let teams = client.all_teams().await?;
    let players: Vec<_> = teams
        .iter()
        .flat_map(|x| {
            x.lineup
                .iter()
                .chain(&x.rotation)
                .chain(&x.bullpen)
                .chain(&x.bench)
                .map(move |y| (x, &*y as &str))
        })
        .collect();
    let mut resp = vec![];
    for chunk in players.chunks(50) {
        let chunk_ids: Vec<&str> = chunk.iter().map(|x| x.1).collect();
        for player in client.players(&chunk_ids).await?.into_iter() {
            let team = chunk
                .iter()
                .find(|x| x.1 == player.id)
                .expect("player missing")
                .0
                .clone();
            resp.push((team, player));
        }
    }
    Ok(resp)
}

async fn team_imp(ctx: &Context, msg: &Message, args: Args, forbidden: bool) -> CommandResult {
    msg.channel_id.broadcast_typing(&ctx.http).await?;
    let client = blased::BlaseballClient::new();
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
async fn team(ctx: &Context, msg: &Message, args: Args) -> CommandResult {
    team_imp(ctx, msg, args, false).await
}

#[command]
async fn team_forbidden(ctx: &Context, msg: &Message, args: Args) -> CommandResult {
    team_imp(ctx, msg, args, true).await
}

async fn player_imp(ctx: &Context, msg: &Message, args: Args, forbidden: bool) -> CommandResult {
    msg.channel_id.broadcast_typing(&ctx.http).await?;
    let player_name = args.rest().to_ascii_lowercase();
    let players = all_players().await?;
    let (team, player) = players
        .into_iter()
        .min_by_key(|x| edit_distance(&player_name, &x.1.name.to_ascii_lowercase()))
        .expect("got no players");
    msg.channel_id
        .send_message(&ctx.http, |m| {
            m.embed(|e| {
                let url = format!("https://www.blaseball.com/player/{}", player.id);
                let team_url = format!("https://www.blaseball.com/team/{}", team.id);
                let emoji_num_str = &team.emoji[2..];
                let emoji_num: u32 =
                    u32::from_str_radix(emoji_num_str, 16).expect("got invalid emoji from db");
                let emoji: char = emoji_num.try_into().expect("got invalid emoji from db");
                let emoji_str = emoji.to_string();
                let emoji_encoded = utf8_percent_encode(&emoji_str, NON_ALPHANUMERIC);
                let png = format!("https://emojicdn.elk.sh/{}?style=twitter", emoji_encoded);
                e.title(player.name)
                    .url(url)
                    .author(|a| a.name(&team.full_name).url(team_url).icon_url(png));

                if let Some(bat) = player.bat.filter(|x| x != "") {
                    e.field("Bat", bat, true);
                }
                if let Some(armor) = player.armor.filter(|x| x != "") {
                    e.field("Armor", armor, true);
                }
                if let Some(ritual) = player.ritual.filter(|x| x != "") {
                    e.field("Pregame Ritual", ritual, true);
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
                    e.field("Coffee Style", styles[coffee], true);
                }
                if let Some(blood) = player.blood {
                    let types = [
                        "A", "AA", "AAA", "Acidic", "Basic", "O", "O No", "H₂O", "Electric",
                        "Love", "Fire", "Psychic", "Grass",
                    ];
                    e.field("Blood Type", types[blood], true);
                }
                e.field("Fate", player.fate, true);

                e
            })
        })
        .await?;
    Ok(())
}

#[command]
async fn player(ctx: &Context, msg: &Message, args: Args) -> CommandResult {
    player_imp(ctx, msg, args, false).await
}

#[command]
async fn player_forbidden(ctx: &Context, msg: &Message, args: Args) -> CommandResult {
    player_imp(ctx, msg, args, true).await
}

#[help]
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
