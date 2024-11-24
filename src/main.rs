use chrono::Timelike;
use serde::{Deserialize, Serialize};
use sqlx::{migrate::MigrateDatabase, SqlitePool};
use std::{
    collections::HashSet,
    env,
    error::Error,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
use teloxide::{
    dispatching::{HandlerExt, UpdateFilterExt},
    dptree,
    prelude::{Dispatcher, *},
    types::{
        CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, InputFile, MediaVideo, Message,
        MessageId, ParseMode, ReactionType, Recipient, ThreadId, Update,
    },
};
use teloxide::{
    net::Download,
    types::{MediaKind, MediaPhoto, MessageCommon, MessageKind, ReplyParameters},
    utils::command::BotCommands,
};
use tokio::fs;
use url::Url;
mod model;
use model::*;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
struct ConfigParameters {
    maintainers: HashSet<UserId>,
    judge_chat: ChatId,
}

async fn init_db(db_url: &str) -> Result<SqlitePool, sqlx::Error> {
    let pool = SqlitePool::connect(db_url)
        .await
        .expect("Failed to connect to database");

    if !sqlx::Sqlite::database_exists(&db_url).await? {
        sqlx::Sqlite::create_database(&db_url).await?;
    }

    Ok(pool)
}

#[derive(BotCommands, Clone)]
#[command(rename_rule = "snake_case", parse_with = "split")]
enum ParticipantCommand {
    #[command(hide)]
    Start,

    #[command(
        description = "Join a team. E.g. /join_team team123",
        parse_with = "default"
    )]
    #[command(alias = "join")]
    JoinTeam(String),
    #[command(description = "Show the team members.")]
    TeamOverview,
    #[command(description = "Shows your team score.")]
    Score,

    // Misc help functions for Spree Break
    #[command(description = "Current safety team and emergency numbers.")]
    EmergencyInformation,
    #[command(description = "Get the survival guide.")]
    SurvivalGuide,
    #[command(description = "Show the schedule.")]
    Schedule,

    /// Shows this message.
    Help,
}

/// Maintainer commands
#[derive(BotCommands, Clone)]
#[command(rename_rule = "snake_case", parse_with = "split")]
enum MaintainerCommands {
    #[command(description = "Enable or disable submissions")]
    EnableSubmissions { status: bool },

    #[command(description = "List teams without team members")]
    ListTeams,
    #[command(description = "List teams and their respective members")]
    ListTeamMembers,
    #[command(description = "Leaderboard")]
    Scoreboard,
    #[command(description = "[CAUTION] List submission for each team")]
    ListTeamSubmissions,
    #[command(description = "[CAUTION] List judged submission for each team")]
    ListTeamSubmissionJudgments,
    #[command(description = "Force update team forums")]
    UpdateTeamForums,

    #[command(description = "Send a message to all users", parse_with = "default")]
    MessageToParticipants(String),

    #[command(description = "List participants")]
    ListParticipants,

    #[command(description = "Rate a submission")]
    Judge { image_ref: i32, challenge: String },

    #[command(description = "[CAUTION] List submissions")]
    ListSubmissions,

    #[command(description = "[CAUTION] List judgements")]
    ListJudgements,
}

fn submission_message(sub: &SubmissionExtended) -> String {
    let datetime = sub.date.to_string();
    format!(
        "Submission from @{} ({} {})\nTeam: {}\nTime: {}\nCaption: {}\nID: {}",
        sub.username.clone().unwrap_or("-".to_owned()),
        sub.first_name,
        sub.last_name.clone().unwrap_or("NO-LASTNAME".to_owned()),
        sub.team,
        datetime,
        Some(sub.caption.clone())
            .map(|x| if x.len() == 0 { "N/P".to_owned() } else { x })
            .unwrap(),
        sub.message_id,
    )
}

async fn update_teams_in_forum(
    bot: &Bot,
    pool: &SqlitePool,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let teams: HashSet<_> =
        sqlx::query_as::<_, Team>("SELECT DISTINCT team, COUNT(*) AS count FROM users")
            .fetch_all(pool)
            .await
            .unwrap()
            .iter()
            .map(|x| x.team.clone())
            .collect();
    let teams_in_forum = sqlx::query_as::<_, Forum>("SELECT DISTINCT id, name FROM forums")
        .fetch_all(pool)
        .await
        .unwrap();

    let forum_team_names: HashSet<_> = teams_in_forum
        .clone()
        .iter()
        .map(|x| x.name.to_owned())
        .collect();
    let forums_to_create: HashSet<_> = teams
        .clone()
        .into_iter()
        .filter(|team| !forum_team_names.contains(team))
        .collect();
    let forums_to_close = teams_in_forum
        .into_iter()
        .filter(|team| !teams.contains(&team.name.clone()))
        .collect::<HashSet<Forum>>();

    let new_teams_futures = forums_to_create.iter().map(|team| async {
        let topic = bot
            .create_forum_topic(
                Recipient::ChannelUsername("@esn_tumi_spreebreak_24ws_admin".to_owned()),
                team.to_owned(),
                7322096,
                "🔥",
            )
            .await?;
        log::warn!("{:?}", topic);

        sqlx::query("INSERT INTO forums (id, name) VALUES ($1, $2)")
            .bind(topic.thread_id.0 .0)
            .bind(team.to_owned())
            .execute(pool)
            .await?;

        log::warn!("Created {:?}", team.to_owned());
        Result::<_, Box<dyn Error + Send + Sync>>::Ok((topic.thread_id.0 .0, team.to_owned()))
    });
    let _ = futures::future::join_all(new_teams_futures).await;

    let close_forum_topics_futures = forums_to_close.iter().map(|thread| async {
        log::warn!("Remove {:?}", thread.to_owned());
        // bot.delete_forum_topic(
        bot.close_forum_topic(
            Recipient::ChannelUsername("@esn_tumi_spreebreak_24ws_admin".to_owned()),
            ThreadId(MessageId(thread.id)),
        )
        .await?;

        // sqlx::query("DELETE FROM forums WHERE id = $1")
        sqlx::query("UPDATE forums SET open = false WHERE id = $1")
            .bind(thread.id)
            .execute(pool)
            .await?;
        log::warn!("Deleted topic {:?}", thread.to_owned());
        Result::<_, Box<dyn Error + Send + Sync>>::Ok(())
    });
    let _ = futures::future::join_all(close_forum_topics_futures).await;

    Ok(())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
enum Media {
    Photo(MediaPhoto),
    Video(MediaVideo),
}

async fn receive_submission(
    media: Media,
    msg: Message,
    bot: Bot,
    cfg: ConfigParameters,
    pool: SqlitePool,
    submissions_enabled: Arc<AtomicBool>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if !submissions_enabled.load(Ordering::Relaxed) {
        bot.send_message(msg.chat.id, "Submissions are currently disabled")
            .await?;
        return Ok(());
    }
    // Check if the user is part of a team
    let user_id = msg.from.as_ref().unwrap().id.0 as i64;
    let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = $1 LIMIT 1")
        .bind(user_id)
        .fetch_optional(&pool)
        .await?;
    if user.is_none() {
        bot.send_message(
            msg.chat.id,
            "You are not part of a team. Use /join_team to join a team.",
        )
        .await?;
        return Ok(());
    }

    let file_id = match media.clone() {
        Media::Photo(photos) => {
            let img = photos.photo.last().expect("Didn't receive any photo(s)");
            let file_id = &img.file; // Get the file ID of the first photo size
            bot.get_file(file_id.id.clone()).await?
        }
        Media::Video(video) => {
            let file_id = &video.video.file; // Get the file ID of the first photo size
            bot.get_file(file_id.id.clone()).await?
        }
    };
    let file = bot.get_file(file_id.id.clone()).await?;

    bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::UploadPhoto)
        .await?;

    let path = format!("./submissions/{}", file.path.replace("/", "_"));
    let mut dst = fs::File::create(path.clone()).await?;
    bot.download_file(&file.path, &mut dst).await?;
    log::info!(
        "Received photo from {:?}",
        msg.from.as_ref().unwrap().full_name()
    );
    log::info!("Photo downloaded: {:?} to `{:?}`", file, path);

    // TODO: This should be retrieved from the database
    // TODO: Team name needs to be taken from databse
    let sub = Submission {
        message_id: msg.id.0 as i64,
        team: "".to_string(),
        date: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
        caption: msg.caption().unwrap_or_default().to_string(),
        r#type: match media {
            Media::Photo(_) => 0,
            Media::Video(_) => 1,
        },
        user: msg.from.clone().unwrap().id.0 as i64,
    };
    let result = sqlx::query(
        "INSERT INTO submissions (message_id, team, date, caption, type, user)
        SELECT $1, team, datetime('now'), $2, $3, $4 FROM users WHERE id = $4", // VALUES ($1, $2, datetime('now'), $3, $4, $5)",
    )
    // TODO: Move to optional fields without setting them to ""
    .bind(sub.message_id)
    // .bind(sub.team)
    .bind(sub.caption)
    .bind(sub.r#type)
    .bind(sub.user)
    .execute(&pool)
    .await?;
    log::trace!("SQL Result {:?}", result);

    // Join the tables users and submissions on the user id
    let sub_ext = sqlx::query_as::<_, SubmissionExtended>(
        "SELECT s.message_id, s.team, u.username, u.first_name, u.last_name, s.date, s.caption, s.type AS type, f.id AS forum_id
        FROM submissions s
        LEFT JOIN users u ON s.user = u.id
        LEFT JOIN forums f ON s.team = f.name
        WHERE s.message_id = $1
        LIMIT 1",
    ).bind(msg.id.0).fetch_one(&pool).await?;
    log::warn!("{:?}", sub_ext);
    if let None = sub_ext.forum_id {
        log::warn!("Did not find associated forum; will create");
    }

    // Forward to judge chat
    let mut forwarded_msg = bot.forward_message(cfg.judge_chat, msg.chat.id, msg.id);
    if let Some(thread_id) = sub_ext.forum_id {
        log::debug!("Forwarding to forum {:?}", thread_id);
        forwarded_msg = forwarded_msg.message_thread_id(ThreadId(MessageId(thread_id)));
    }
    let forwarded_msg = forwarded_msg.await?;

    bot.send_message(cfg.judge_chat, submission_message(&sub_ext))
        .reply_parameters(ReplyParameters::new(forwarded_msg.id))
        .disable_notification(true)
        .await?;

    // Select challenges from the table challenges that have not yet been completed by the team of user with user id = sub.user
    let remaining_challenges = sqlx::query_as::<_, Challenge>(
        "SELECT name, short_name
        FROM challenges
        WHERE name NOT IN (
            SELECT challenge_name
            FROM judgement j
            LEFT JOIN submissions s ON j.submission_id = s.message_id
            WHERE s.team = (
                SELECT team
                FROM users
                WHERE id = $1))",
    )
    .bind(sub.user)
    .fetch_all(&pool)
    .await?;

    let keyboard = make_keyboard(
        msg.from.unwrap().id.0.to_string(),
        msg.id.0.to_string(),
        remaining_challenges,
    );
    let mut response = bot
        .send_message(cfg.judge_chat, "Select challenge or action")
        .reply_markup(keyboard)
        .disable_notification(true);
    if let Some(thread_id) = sub_ext.forum_id {
        response = response.message_thread_id(ThreadId(MessageId(thread_id)));
    }
    response.await?;

    Ok(())
}

async fn maintainer_commands(
    msg: Message,
    bot: Bot,
    cmd: MaintainerCommands,
    pool: SqlitePool,
    lock: Arc<Mutex<()>>,
    submissions_enabled: Arc<AtomicBool>,
    cfg: ConfigParameters,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    match cmd {
        MaintainerCommands::ListTeams => {
            let res =
                sqlx::query_as::<_, Team>("SELECT DISTINCT team, COUNT(*) as count FROM users")
                    .fetch_all(&pool)
                    .await
                    .unwrap();
            let teams = res
                .into_iter()
                .map(|x| format!("- {} (#{})", x.team, x.count))
                .collect::<Vec<String>>()
                .join("\n");

            bot.send_message(msg.chat.id, format!("Teams:\n{}", teams))
                .await?;
            Ok(())
        }
        MaintainerCommands::ListTeamMembers => {
            let res = sqlx::query_as::<_, User>("SELECT * FROM users ORDER BY team")
                .fetch_all(&pool)
                .await
                .unwrap();
            let users = res
                .iter()
                .map(|x| format!("- {} (#{}) -> {}", x.to_string(), x.id, x.team))
                .collect::<Vec<String>>()
                .join("\n");

            bot.send_message(msg.chat.id, format!("Participants:\n{}", users))
                .await?;
            Ok(())
        }
        MaintainerCommands::Scoreboard => {
            // List teams and their scores
            let res = sqlx::query_as::<_, TeamScore>(
                "SELECT s.team, SUM(j.points) as score
                FROM judgement j
                LEFT JOIN submissions s ON j.submission_id = s.message_id
                LEFT JOIN users u ON s.team = u.team
                WHERE j.valid = 1
                GROUP BY s.team ORDER BY score DESC",
            )
            .fetch_all(&pool)
            .await?;
            let scores = res
                .iter()
                .enumerate()
                .map(|(place, x)| format!("{}. `{}` with {} pts.", place + 1, x.team, x.score))
                .collect::<Vec<String>>()
                .join("\n");
            bot.send_message(msg.chat.id, format!("Scoreboard:\n{}", scores))
                .await?;
            Ok(())
        }
        MaintainerCommands::ListTeamSubmissions => {
            let res = sqlx::query_as::<_, TeamScore>(
                "SELECT s.team, SUM(j.points) as score
                FROM judgement j
                LEFT JOIN submissions s ON j.submission_id = s.message_id
                LEFT JOIN users u ON s.team = u.team
                WHERE j.valid = 1
                GROUP BY s.team ORDER BY score DESC",
            )
            .fetch_all(&pool)
            .await?;
            for team in res {
                let submissions = sqlx::query_as::<_, SubmissionExtended>(
                    "SELECT s.message_id, s.team, u.username, u.first_name, u.last_name, s.date, s.caption, s.type AS type, 0 as forum_id
                    FROM submissions s
                    LEFT JOIN users u ON s.user = u.id
                    WHERE s.team = $1",
                )
                .bind(team.clone().team)
                .fetch_all(&pool)
                .await?;
                let submissions = submissions
                    .iter()
                    .map(|x| submission_message(x))
                    .collect::<Vec<String>>()
                    .join("\n\n");
                bot.send_message(
                    msg.chat.id,
                    format!("Submissions for team `{}`:\n{}", team.team, submissions),
                )
                .await?;
            }
            Ok(())
        }
        MaintainerCommands::ListTeamSubmissionJudgments => {
            let res = sqlx::query_as::<_, TeamScore>(
                "SELECT s.team, SUM(j.points) as score
                FROM judgement j
                LEFT JOIN submissions s ON j.submission_id = s.message_id
                LEFT JOIN users u ON s.team = u.team
                WHERE j.valid = 1
                GROUP BY s.team ORDER BY score DESC",
            )
            .fetch_all(&pool)
            .await?;
            for team in res {
                let judgements = sqlx::query_as::<_, Judgement>(
                    "SELECT j.submission_id, j.challenge_name, j.points, j.valid
                    FROM judgement j
                    LEFT JOIN submissions s ON j.submission_id = s.message_id
                    WHERE s.team = $1",
                )
                .bind(team.clone().team)
                .fetch_all(&pool)
                .await?;
                let judgements = judgements
                    .iter()
                    .map(|x| {
                        format!(
                            "- ref=`{}` challenge=`{}` pts={} valid={}",
                            x.submission_id, x.challenge_name, x.points, x.valid
                        )
                    })
                    .collect::<Vec<String>>()
                    .join("\n");
                bot.send_message(
                    msg.chat.id,
                    format!("Judgements for team `{}`:\n{}", team.team, judgements),
                )
                .await?;
            }
            Ok(())
        }
        MaintainerCommands::UpdateTeamForums => {
            let _guard = lock.lock().await;
            update_teams_in_forum(&bot, &pool).await?;
            Ok(())
        }
        MaintainerCommands::EnableSubmissions { status } => {
            submissions_enabled.store(status, Ordering::Relaxed);
            Ok(())
        }
        MaintainerCommands::ListParticipants => {
            let users = sqlx::query_as::<_, User>("SELECT * FROM users")
                .fetch_all(&pool)
                .await
                .unwrap();
            let users = users
                .iter()
                .map(|x| format!("- {} (#{})", x.to_string(), x.id))
                .collect::<Vec<String>>()
                .join("\n");

            bot.send_message(msg.chat.id, format!("Participants:\n{}", users))
                .await?;
            Ok(())
        }
        MaintainerCommands::MessageToParticipants(message) => {
            if message.is_empty() {
                bot.send_message(msg.chat.id, "Broadcast error: Empty message")
                    .await?;
                return Ok(());
            }
            // Query over all users and send a message to each of them
            let users = sqlx::query_as::<_, User>("SELECT * FROM users")
                .fetch_all(&pool)
                .await
                .unwrap();
            for user in users {
                if cfg.maintainers.contains(&UserId(user.id as u64)) {
                    if msg.from.as_ref().unwrap().id.0 == user.id as u64 {
                        continue;
                    } else {
                        bot.send_message(
                            UserId(user.id as u64),
                            format!("Broadcast from {}", msg.from.as_ref().unwrap().full_name()),
                        )
                        .await?;
                    }
                }
                bot.send_message(UserId(user.id as u64), message.clone())
                    .await?;
            }
            bot.send_message(msg.chat.id, "Message sent").await?;
            Ok(())
        }
        MaintainerCommands::Judge {
            image_ref: submission_ref,
            challenge,
        } => {
            // Retrieve the associate aka user who submitted the submission from the sql
            let associate = sqlx::query_as::<_, User>(
                "SELECT u.id, u.team, u.username, u.first_name, u.last_name
                FROM submissions s
                LEFT JOIN users u ON s.user = u.id
                WHERE s.message_id = $1",
            )
            .bind(submission_ref)
            .fetch_optional(&pool)
            .await?;
            // Check that challenge exists
            let challenge = match challenge.as_str() {
                // TODO: Handle this in a better way
                "___unclear" => Some(Challenge {
                    name: "___unclear".to_owned(),
                    short_name: "Unclear".to_owned(),
                }),
                "___invalid" => Some(Challenge {
                    name: "___invalid".to_owned(),
                    short_name: "Invalid".to_owned(),
                }),
                _ => {
                    sqlx::query_as::<_, Challenge>(
                        "SELECT name, short_name
                FROM challenges
                WHERE name = $1",
                    )
                    .bind(challenge)
                    .fetch_optional(&pool)
                    .await?
                }
            };
            match (associate, challenge) {
                (Some(user), Some(challenge)) => {
                    judge(
                        user.id.to_string(),
                        submission_ref.to_string(),
                        challenge.name,
                        &bot,
                        &pool,
                    )
                    .await?;

                    bot.send_message(msg.chat.id, "Submission successfully judged")
                        .await?;
                }
                (_, None) => {
                    bot.send_message(msg.chat.id, "Challenge not found").await?;
                }
                (None, _) => {
                    bot.send_message(msg.chat.id, "Submission not found")
                        .await?;
                }
            }

            Ok(())
        }
        MaintainerCommands::ListSubmissions => {
            let submissions = sqlx::query_as::<_, SubmissionExtended>("  
                SELECT s.message_id, s.team, u.username, u.first_name, u.last_name, s.date, s.caption, s.type AS type, 0 as forum_id
                FROM submissions s
                LEFT JOIN users u ON s.user = u.id").fetch_all(&pool).await?;
            let submissions = submissions
                .iter()
                .map(|x| submission_message(x))
                .collect::<Vec<String>>()
                .join("\n");
            bot.send_message(msg.chat.id, format!("Submissions: {}", submissions))
                .await?;
            Ok(())
        }
        MaintainerCommands::ListJudgements => {
            let judgements = sqlx::query_as::<_, Judgement>("SELECT * FROM judgement")
                .fetch_all(&pool)
                .await?;
            let judgements = judgements
                .iter()
                .map(|x| {
                    format!(
                        "- ref=`{}` challenge=`{}` pts={} valid={}",
                        x.submission_id, x.challenge_name, x.points, x.valid
                    )
                })
                .collect::<Vec<String>>()
                .join("\n");
            bot.send_message(msg.chat.id, format!("Judgements: {}", judgements))
                .await?;
            Ok(())
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    pretty_env_logger::init();
    let db_url: String = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let judge_chat: String = env::var("JUDGE_CHAT_ID").expect("JUDGE_CHAT_ID not set");
    let judge_chat = ChatId(judge_chat.parse::<i64>().unwrap());

    let maintainers: String = env::var("MAINTAINERS").expect("MAINTAINERS not set");
    let maintainers = maintainers
        .split(",")
        .map(|x| x.parse::<u64>().unwrap())
        .map(UserId)
        .collect::<HashSet<UserId>>();

    let bot = Bot::from_env();
    let db = init_db(&db_url)
        .await
        .expect("Failed to initialize database");

    let parameters = ConfigParameters {
        judge_chat: judge_chat,
        maintainers: maintainers,
    };

    let lock = Arc::new(Mutex::new(()));
    let submissions_enabled = Arc::new(AtomicBool::new(true));

    let handler = Update::filter_message()
        .branch(
            dptree::entry()
                .filter_command::<ParticipantCommand>()
                .filter(|msg: Message, cfg: ConfigParameters| {
                    !(msg.chat.is_group() || msg.chat.is_supergroup())
                        || msg.chat.id == cfg.judge_chat
                })
                .branch(
                    // Handle join team separately
                    dptree::filter(|cmd: ParticipantCommand| {
                        matches!(cmd, ParticipantCommand::JoinTeam(_))
                    })
                    .endpoint(join_team),
                )
                .branch(
                    dptree::entry()
                        .filter_command::<ParticipantCommand>()
                        .endpoint(participant_commands_handler),
                ),
        )
        .branch(
            // Filter a maintainer by a user ID
            dptree::filter(|cfg: ConfigParameters, msg: Message| {
                msg.from
                    .map(|user| cfg.maintainers.contains(&user.id) && msg.chat.is_private())
                    .unwrap_or_default()
            })
            .filter_command::<MaintainerCommands>()
            .endpoint(maintainer_commands),
        )
        .branch(
            // Filter a media messages for submission
            dptree::filter(|cfg: ConfigParameters, msg: Message| {
                msg.chat.is_private()
                    && !msg.chat.is_group()
                    && !msg.chat.is_supergroup()
                    && msg.chat.id != cfg.judge_chat
            })
            .filter_map(|msg: Message| match msg.kind {
                MessageKind::Common(MessageCommon {
                    media_kind: MediaKind::Photo(ref photos),
                    ..
                }) => Some(Media::Photo(photos.clone())),
                MessageKind::Common(MessageCommon {
                    media_kind: MediaKind::Video(ref video),
                    ..
                }) => Some(Media::Video(video.clone())),
                _ => None,
            })
            .endpoint(receive_submission),
        )
        .branch(
            dptree::filter(|msg: Message, cfg: ConfigParameters| msg.chat.id != cfg.judge_chat)
                .endpoint(|bot: Bot, msg: Message| async move {
                    if msg.chat.is_group() || msg.chat.is_supergroup() {
                        bot.send_message(msg.chat.id, "Please use me in a private chat")
                            .await?;
                        return Ok(());
                    }

                    if let Some(text) = msg.text() {
                        // Some easter eggs
                        let response = match text.to_lowercase().as_str() {
                            t if t.contains("beer") || t.contains("bier") => {
                                "I love Bavarian beer!"
                            }
                            t if t.contains("prost") => "Prost!",
                            t if t.contains("servus")
                                || t.contains("hallo")
                                || t.contains("hi")
                                || t.contains("hey") =>
                            {
                                "Servus!"
                            }
                            _ => "Sorry, I didn't understand your message. /help",
                        };
                        bot.send_message(msg.chat.id, response).await?;
                    } else {
                        bot.send_message(
                            msg.chat.id,
                            "Sorry, this type of message isn't supported.",
                        )
                        .await?;
                    }
                    Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
                }),
        );

    let meta_handler = dptree::entry()
        .branch(handler)
        .branch(Update::filter_callback_query().endpoint(callback_handler));

    Dispatcher::builder(bot, meta_handler)
        .dependencies(dptree::deps![db, parameters, lock, submissions_enabled])
        .default_handler(|upd| async move {
            log::warn!("Unhandled update: {:?}", upd);
        })
        .error_handler(LoggingErrorHandler::with_custom_text(
            "An error has occurred in the dispatcher",
        ))
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
    Ok(())
}

fn make_keyboard(
    associate: String,
    reference: String,
    challenges: Vec<Challenge>,
) -> InlineKeyboardMarkup {
    let mut keyboard: Vec<Vec<InlineKeyboardButton>> = vec![];

    for versions in challenges.chunks(1) {
        let row = versions
            .iter()
            .map(|challenge| {
                InlineKeyboardButton::callback(
                    challenge.short_name.to_owned(),
                    format!("{}###{}###{}", associate, reference, challenge.name),
                )
            })
            .collect();

        keyboard.push(row);
    }
    keyboard.push(vec![
        InlineKeyboardButton::callback(
            "⚠️ Unclear",
            format!("{}###{}###___unclear", associate, reference),
        ),
        InlineKeyboardButton::callback(
            "❌ Invalid",
            format!("{}###{}###___invalid", associate, reference),
        ),
    ]);

    InlineKeyboardMarkup::new(keyboard)
}

async fn join_team(
    bot: Bot,
    msg: Message,
    cmd: ParticipantCommand,
    lock: Arc<Mutex<()>>,
    pool: SqlitePool,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    match cmd {
        ParticipantCommand::JoinTeam(team) => {
            if team.trim().len() == 0 {
                bot.send_message(
                    msg.chat.id,
                    "Please provide a team name. /join_team followed by the team name",
                )
                .await?;
                return Ok(());
            }
            let data = User {
                id: msg.from.as_ref().unwrap().id.0 as i64,
                team: team.to_owned(),
                username: msg.from.as_ref().unwrap().username.clone(),
                first_name: msg.from.as_ref().unwrap().first_name.clone(),
                last_name: msg.from.as_ref().unwrap().last_name.clone(),
            };
            let result = sqlx::query(
                "INSERT INTO users (id, team, username, first_name, last_name, created_at)
                VALUES ($1, $2, $3, $4, $5, datetime('now'))
                ON CONFLICT(id) DO UPDATE SET team = excluded.team",
            )
            .bind(data.id)
            .bind(data.team)
            .bind(data.username)
            .bind(data.first_name)
            .bind(data.last_name)
            .execute(&pool)
            .await;
            result.unwrap();
            bot.send_message(msg.chat.id, format!("You joined team `{}`\n\nCheck the team members with /team\\_overview\\.\nDon't change your team \\(name\\) after the first submisssion; previous submissions will not count anymore", team))
                .parse_mode(ParseMode::MarkdownV2)
                .await?;

            let _guard = lock.lock().await;
            update_teams_in_forum(&bot, &pool).await?;
        }
        _ => {
            unreachable!()
        }
    };
    Ok(())
}

async fn participant_commands_handler(
    cfg: ConfigParameters,
    bot: Bot,
    me: teloxide::types::Me,
    msg: Message,
    cmd: ParticipantCommand,
    pool: SqlitePool,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if (msg.chat.is_group() || msg.chat.is_supergroup()) && !matches!(cmd, ParticipantCommand::Help)
    {
        bot.send_message(msg.chat.id, "Please use me in a private chat")
            .await?;
        return Ok(());
    }
    match cmd {
        ParticipantCommand::Start => {
            bot.send_message(
                msg.chat.id,
                format!("Hello {}", msg.chat.first_name().unwrap_or("Spree Breaker")),
            )
            .await?;
            bot.send_message(
                msg.chat.id,
                "Check /help for ways that I can provide you help.\n\nTo get started with the photo challenge use /join_team followed by the team name. The team name must be identical for all team members.\n\nAny photos or videos you sent me will be submissions to photo challenge. Please consider adding meaningful captions!"
            )
            .await?;
        }
        ParticipantCommand::Help => {
            let text = if cfg.maintainers.contains(&msg.from.unwrap().id) {
                format!(
                    "{}\n\n{}",
                    ParticipantCommand::descriptions(),
                    MaintainerCommands::descriptions()
                )
            } else if msg.chat.is_group() || msg.chat.is_supergroup() {
                ParticipantCommand::descriptions()
                    .username_from_me(&me)
                    .to_string()
            } else {
                ParticipantCommand::descriptions().to_string()
            };
            bot.send_message(msg.chat.id, text).await?;
        }
        ParticipantCommand::JoinTeam(_team) => {
            unreachable!("This should be handled by the join_team function");
        }
        ParticipantCommand::TeamOverview => {
            let team_members = sqlx::query_as::<_, User>(
                "SELECT * FROM users WHERE team = (SELECT team FROM users WHERE id = $1)",
            )
            .bind(msg.from.as_ref().unwrap().id.0 as i64)
            .fetch_all(&pool)
            .await?;

            let team = sqlx::query_as::<_, Team>(
                "SELECT team, COUNT(*) AS count FROM users WHERE id = $1 LIMIT 1",
            )
            .bind(msg.from.as_ref().unwrap().id.0 as i64)
            .fetch_one(&pool)
            .await?;

            if team.count > 0 {
                let team_members_text = if team_members.is_empty() {
                    "No team members yet".to_owned()
                } else {
                    team_members
                        .iter()
                        .map(|x| format!("- {}", x.to_string()))
                        .collect::<Vec<String>>()
                        .join("\n")
                };
                log::warn!("{:?}", team);
                bot.send_message(
                    msg.chat.id,
                    format!(
                        "Overview team <code>{}</code>\n\n{} Member(s):\n{team_members_text}",
                        team.team,
                        team_members.len()
                    ),
                )
                .parse_mode(ParseMode::Html)
                .await?;
            } else {
                bot.send_message(msg.chat.id, "You are not yet part of a team")
                    .await?;
            }
        }
        ParticipantCommand::Score => {
            let user_id = msg.from.as_ref().unwrap().id.0 as i64;
            #[derive(sqlx::FromRow, Debug)]
            struct ChallengeExtended {
                challenge_name: String,
                points: i32,
            }
            // Join over the tables users, submissions and judgement for the user_id
            let res = sqlx::query_as::<_, ChallengeExtended>(
                "SELECT j.challenge_name, j.points
                FROM judgement j
                LEFT JOIN submissions s ON j.submission_id = s.message_id
                LEFT JOIN users u ON s.team = u.team
                WHERE u.id = $1 AND j.valid = 1",
            )
            .bind(user_id)
            .fetch_all(&pool)
            .await?;
            let scores = res
                .into_iter()
                .map(|x| format!("- {} +{} pts.", x.challenge_name, x.points))
                .collect::<Vec<String>>()
                .join("\n");

            #[derive(sqlx::FromRow, Debug)]
            struct Score {
                score: i32,
            }
            let res = sqlx::query_as::<_, Score>(
                "SELECT SUM(j.points) as score
                    FROM judgement j
                    LEFT JOIN submissions s ON j.submission_id = s.message_id
                    LEFT JOIN users u ON s.team = u.team
                    WHERE u.id = $1 AND j.valid = 1",
            )
            .bind(user_id)
            .fetch_one(&pool)
            .await?;
            // Get the number of submissions of the team of the current user and how many of them appear in the table judgement
            let res_submissions = sqlx::query_as::<_, Score>(
                "SELECT COUNT(*) as score
                    FROM submissions s
                    LEFT JOIN users u ON s.team = u.team
                    WHERE u.id = $1",
            )
            .bind(user_id)
            .fetch_one(&pool)
            .await?;
            bot.send_message(
                msg.chat.id,
                format!(
                    "{scores}\n\nTotal score from {} submissions: {}",
                    res_submissions.score, res.score
                ),
            )
            .await?;
        }
        ParticipantCommand::Schedule => {
            let source = sqlx::query_as::<_, Config>(
                "SELECT name, value FROM config WHERE name = 'schedule_source'",
            )
            .fetch_optional(&pool)
            .await?
            .unwrap_or(Config {
                name: "schedule_source".to_owned(),
                value: "file::assets/schedule.png".to_owned(),
            });
            log::trace!("Load schedule config = {:?}", source);
            let parts: Vec<&str> = source.value.split("::").collect();
            let (mode, path) = (parts[0], parts[1]);
            let file = match mode {
                "file" => InputFile::file(Path::new(path)),
                "url" => InputFile::url(Url::parse(path)?),
                _ => unimplemented!("Unknown mode"),
            };
            bot.send_photo(msg.chat.id, file).await?;
        }
        ParticipantCommand::SurvivalGuide => {
            let source = sqlx::query_as::<_, Config>(
                "SELECT name, value FROM config WHERE name = 'city_guide'",
            )
            .fetch_optional(&pool)
            .await?
            .unwrap_or(Config {
                name: "schedule_source".to_owned(),
                value: "file::assets/survival_guide.pdf".to_owned(),
            });
            log::trace!("Load schedule config = {:?}", source);
            let parts: Vec<&str> = source.value.split("::").collect();
            let (mode, path) = (parts[0], parts[1]);
            let file = match mode {
                "file" => InputFile::file(Path::new(path)),
                "url" => InputFile::url(Url::parse(path)?),
                _ => unimplemented!("Unknown mode"),
            };
            bot.send_document(msg.chat.id, file).await?;
        }
        ParticipantCommand::EmergencyInformation => {
            #[derive(sqlx::FromRow, Debug)]
            struct SafetyTeam {
                name: String,
                phone: String,
            }
            // If the hour is before 6am substract 24 from Utc::now then format the date
            let now = chrono::Utc::now();
            let now = if now.hour() < 6 {
                log::trace!("Safety team: before 6am, subtract 1 day");
                now - chrono::Duration::hours(24)
            } else {
                now
            };
            let current_date = now.format("%Y-%m-%d").to_string();
            log::trace!("Current date = {:?}", current_date);

            let team = sqlx::query_as::<_, SafetyTeam>(
                "SELECT name, phone FROM safety_team WHERE date = $1",
            )
            .bind(current_date)
            .fetch_all(&pool)
            .await?;
            log::trace!("Safety team = {:?}", team);
            let team_list = if team.is_empty() {
                "No safety team available right now".to_owned()
            } else {
                team.iter()
                    .map(|x| format!("{}: {}", x.name, x.phone))
                    .collect::<Vec<String>>()
                    .join("\n")
            };
            bot.send_message(msg.chat.id, format!("Our safety team right now. Do not hesitate to talk to any other tutors.\n{team_list}\n\n🚑 <b>Fire brigade & ambulance: +112</b>\n👮 Police: +110")).parse_mode(ParseMode::Html).await?;
        }
    };
    Ok(())
}

async fn callback_handler(
    bot: Bot,
    pool: SqlitePool,
    q: CallbackQuery,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(raw_choice) = q.data {
        let parts = raw_choice.split("###").collect::<Vec<&str>>();
        assert_eq!(parts.len(), 3);
        let (associate, image_ref, choice) = (parts[0], parts[1], parts[2]);
        log::debug!(
            "Received callback (raw {:?}) assoc={:?} ref={:?} choice={:?}",
            associate,
            raw_choice,
            image_ref,
            choice
        );

        let mut callback_query = bot.answer_callback_query(q.id);
        callback_query.show_alert = Some(true);
        callback_query.text = Some(format!("Choice = {}", choice).clone());
        callback_query.await?;

        judge(
            associate.to_owned(),
            image_ref.to_owned(),
            choice.to_owned(),
            &bot,
            &pool,
        )
        .await?;

        // Edit text of the message to which the buttons were attached
        let text =
            format!("Decision <b>{choice}</b>\n\nOverwrite with '/judge {image_ref} [challenge]'");
        if let Some(message) = q.message {
            bot.edit_message_text(message.chat().id, message.id(), text)
                .parse_mode(ParseMode::Html)
                .await?;
        } else if let Some(id) = q.inline_message_id {
            bot.edit_message_text_inline(id, text)
                .parse_mode(ParseMode::Html)
                .await?;
        }

        log::info!("Judge chose: {}", choice);
    }

    Ok(())
}

async fn judge(
    associate: String,
    submission_ref: String,
    challenge: String,
    bot: &Bot,
    pool: &SqlitePool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut points = 1;
    let mut valid = true;
    if challenge == "___unclear" || challenge == "___invalid" {
        points = 0;
        valid = false;
    }

    sqlx::query("INSERT INTO judgement (submission_id, challenge_name, points, valid) VALUES ($1, $2, $3, $4) ON CONFLICT(submission_id) DO UPDATE SET challenge_name = excluded.challenge_name")
            .bind(submission_ref.clone())
            .bind(challenge.clone())
            .bind(points)
            .bind(valid)
            .execute(pool)
            .await?;

    // All of this can fail since the user might have deleted their message
    // TODO: Handle deleted messages better, don't just ignore
    if valid == false {
        bot.send_message(
            UserId(associate.parse::<u64>().unwrap()),
            match challenge.as_str() {
                "___unclear" => "Please resend your submission with a clear caption",
                "___invalid" => "Your submission is invalid",
                _ => unreachable!(),
            },
        )
        .reply_parameters(ReplyParameters::new(MessageId(
            submission_ref.parse::<i32>().unwrap(),
        )))
        .await?;
        // Clear existing reactions
        bot.set_message_reaction(
            UserId(associate.parse::<u64>().unwrap()),
            MessageId(submission_ref.parse::<i32>().unwrap()),
        )
        .erase()
        .await?;
    } else {
        bot.set_message_reaction(
            UserId(associate.parse::<u64>().unwrap()),
            MessageId(submission_ref.parse::<i32>().unwrap()),
        )
        .reaction(vec![ReactionType::Emoji {
            emoji: "❤".to_owned(),
        }])
        .await?;
    }

    Ok(())
}
