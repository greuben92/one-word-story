use std::collections::HashSet;
use std::env;
use std::fs;
use std::sync::Arc;
use tokio::sync::RwLock;

use censor::Censor;
use serenity::async_trait;
use serenity::model::{
    channel::Message,
    gateway::{GatewayIntents, Ready},
    permissions::Permissions,
    prelude::*,
};
use serenity::prelude::*;

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _ctx: Context, ready: Ready) {
        println!("{} is connected!", ready.user.name);
    }

    async fn message(&self, ctx: Context, msg: Message) {
        if msg.author.bot {
            return;
        }

        if let Some(cmd) = parse_command(&msg) {
            match cmd {
                Ok(cmd) => {
                    run_command(cmd, &msg, &ctx).await;
                }
                Err(error) => {
                    if let Err(why) = msg.reply(&ctx.http, error).await {
                        println!("Error replying: {:?}", why);
                    }
                }
            };
            return;
        }

        parse_message(&ctx, &msg).await;
    }
}

async fn parse_message(ctx: &Context, msg: &Message) {
    let lock = {
        let data = ctx.data.read().await;
        data.get::<ConfigContainer>()
            .expect("Expected ConfigContainer in TypeMap")
            .clone()
    };
    let channel_id = lock.read().await.channel_id;
    // println!("{:?}", channel_id);
    if msg.channel_id != channel_id {
        return;
    }

    if "." == msg.content {
        generate_story(ctx, msg).await;
        return;
    }

    let lock = {
        let data = ctx.data.read().await;
        data.get::<CensorContainer>()
            .expect("Expected CensorContaineronfigContainer in TypeMap")
            .clone()
    };
    let censor = lock.read().await;

    if !is_valid_message(&msg.content, &censor).await {
        if let Err(why) = msg.delete(&ctx.http).await {
            println!("Error replying: {:?}", why);
        }
    }
}

async fn is_valid_message(msg: &str, censor: &Censor) -> bool {
    let words: Vec<&str> = msg.split_whitespace().collect();

    if words.len() > 2 {
        return false;
    }

    if words.len() == 2 && !(words[0].len() <= 2 || words[1].len() <= 2) {
        return false;
    }

    if censor.check(msg) {
        return false;
    }

    true
}

async fn generate_story(ctx: &Context, msg: &Message) {
    // Get up to 250 words.
    let req = msg
        .channel_id
        .messages(&ctx.http, |r| r.before(msg.id).limit(250))
        .await;

    if let Ok(messages) = req {
        let mut char_count = 0;
        let mut title = "Story so far";
        let mut story: Vec<String> = Vec::new();

        for m in messages.iter() {
            if "." == m.content {
                break;
            }

            if m.author.bot {
                continue;
            }

            char_count += m.content.len() + 1; // +1 for space
            if char_count > 4096 {
                send_story(ctx, msg, &mut story, title).await;
                char_count = m.content.len();
                story.clear();
                story.push(m.content.clone());
                title = "continued";
                continue;
            }

            story.push(m.content.clone());
        }
        send_story(ctx, msg, &mut story, title).await;
    };
}

async fn send_story(ctx: &Context, msg: &Message, story: &mut [String], title: &str) {
    if story.is_empty() {
        return;
    }

    story.reverse();
    match msg
        .channel_id
        .send_message(&ctx.http, |m| {
            m.embed(|e| e.title(title).description(story.join(" ")))
        })
        .await
    {
        Ok(m) => {
            if let Err(why) = m.pin(&ctx.http).await {
                println!("Failed to pin message {:?}", why);
            }
        }
        Err(why) => println!("Error generating story: {:?}", why),
    };
}

#[derive(Debug)]
enum Command {
    SetChannel(ChannelId),
    BanWord(String),
    UnbanWord(String),
}

fn parse_command(msg: &Message) -> Option<Result<Command, &'static str>> {
    if !msg.content.starts_with("one-word") {
        return None;
    }

    let words: Vec<&str> = msg.content.split_whitespace().collect();
    if words.len() < 3 {
        return Some(Err("Usage: one-word <set-channel|ban|unban> <arg>"));
    }

    let arg = words[2].to_string();

    match words[1].to_lowercase().as_str() {
        "set-channel" => {
            let channel_id = arg
                .replace("<#", "")
                .replace('>', "")
                .parse::<u64>()
                .map(ChannelId);
            match channel_id {
                Ok(id) => Some(Ok(Command::SetChannel(id))),
                _ => Some(Err("Invalid channel")),
            }
        }
        "ban" => Some(Ok(Command::BanWord(arg))),
        "unban" => Some(Ok(Command::UnbanWord(arg))),
        _ => Some(Err("Invalid command")),
    }
}

async fn run_command(cmd: Command, msg: &Message, ctx: &Context) {
    if !msg_member_has_perm(ctx, msg, Permissions::ADMINISTRATOR).await {
        if let Err(why) = msg
            .reply(&ctx.http, "Only admins are allowed to update settings.")
            .await
        {
            println!("Error replying: {:?}", why);
        }
        return;
    }

    match cmd {
        Command::SetChannel(id) => {
            set_config(ctx, |config: &mut Config| {
                config.channel_id = id;
            })
            .await;
        }
        Command::BanWord(word) => {
            set_config(ctx, |config| {
                config.banned_words.insert(word);
            })
            .await;
        }
        Command::UnbanWord(word) => {
            set_config(ctx, |config| {
                config.banned_words.remove(&word);
            })
            .await;
        }
    };

    if let Err(why) = msg.reply(&ctx.http, "Settings updated").await {
        println!("Error replying: {:?}", why);
    }
}

async fn msg_member_has_perm(ctx: &Context, msg: &Message, required_perm: Permissions) -> bool {
    if let Ok(member) = msg.member(&ctx.http).await {
        if let Ok(perms) = member.permissions(&ctx.cache) {
            return perms.contains(required_perm);
        }
    }

    false
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
struct Config {
    channel_id: ChannelId,
    banned_words: HashSet<String>,
}

struct ConfigContainer;
impl TypeMapKey for ConfigContainer {
    type Value = Arc<RwLock<Config>>;
}

async fn set_config<F>(ctx: &Context, update: F)
where
    F: FnOnce(&mut Config),
{
    let lock = {
        let data = ctx.data.read().await;
        data.get::<ConfigContainer>()
            .expect("Expected ConfigContainer in TypeMap")
            .clone()
    };
    {
        let mut config = lock.write().await;
        update(&mut config);

        let mut data = ctx.data.write().await;
        let censor = censor::Censor::Custom(config.banned_words.clone());
        data.insert::<CensorContainer>(Arc::new(RwLock::new(censor)));

        match env::var("CONFIG_FILE") {
            Ok(path) => {
                let c = config.clone();
                if let Err(why) = fs::write(path, serde_json::to_string(&c).unwrap()) {
                    println!("Error wriring config {:?}", why);
                }
            }
            _ => {
                println!("Mising CONFIG_FILE env. Configuration not saved.");
            }
        };
    }
}

struct CensorContainer;
impl TypeMapKey for CensorContainer {
    type Value = Arc<RwLock<Censor>>;
}

fn read_config() -> Option<Config> {
    match env::var("CONFIG_FILE") {
        Ok(path) => match fs::read_to_string(path) {
            Ok(contents) => match serde_json::from_str::<Config>(&contents) {
                Ok(config) => Some(config),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

#[tokio::main]
async fn main() {
    let token = env::var("DISCORD_TOKEN").expect("Missing discord token.");
    let intents = GatewayIntents::GUILDS
        | GatewayIntents::GUILD_MEMBERS
        | GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;
    let mut client = Client::builder(token, intents)
        .event_handler(Handler)
        .await
        .expect("Error creating client");

    {
        let mut data = client.data.write().await;
        let default_config = read_config().unwrap_or_else(|| Config {
            channel_id: ChannelId(0),
            banned_words: HashSet::new(),
        });
        data.insert::<ConfigContainer>(Arc::new(RwLock::new(default_config.clone())));

        let censor = censor::Censor::Custom(default_config.banned_words);
        data.insert::<CensorContainer>(Arc::new(RwLock::new(censor)));
    };

    if let Err(why) = client.start().await {
        println!("Client error: {:?}", why);
    }
}
