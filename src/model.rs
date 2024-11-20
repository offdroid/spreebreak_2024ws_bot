#[derive(sqlx::FromRow, Debug, Clone)]
pub struct Submission {
    pub message_id: i64,
    pub team: String,
    pub user: i64,
    pub date: String,
    pub caption: String,
    pub r#type: i32,
}

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct SubmissionExtended {
    pub message_id: i64,
    pub team: String,
    pub username: Option<String>,
    pub first_name: String,
    pub last_name: Option<String>,
    pub date: String,
    pub caption: String,
    pub r#type: i32,
    pub forum_id: Option<i32>,
}

#[derive(sqlx::FromRow, Debug)]
pub struct Team {
    pub team: String,
    pub count: i64,
}
impl ToString for Team {
    fn to_string(&self) -> String {
        self.team.clone()
    }
}

#[derive(sqlx::FromRow, Debug, Clone, Eq, PartialEq, Hash)]
pub struct Forum {
    pub id: i32,
    pub name: String,
}

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct Challenge {
    pub name: String,
    pub short_name: String,
}

#[derive(sqlx::FromRow, Debug)]
pub struct User {
    pub id: i64,
    pub team: String,
    pub username: Option<String>,
    pub first_name: String,
    pub last_name: Option<String>,
}
impl ToString for User {
    fn to_string(&self) -> String {
        let name = if let Some(last_name) = &self.last_name {
            format!("{} {}", self.first_name, last_name)
        } else {
            self.first_name.to_owned()
        };

        if let Some(username) = &self.username {
            format!("{} @{}", &name, &username)
        } else {
            name
        }
    }
}

#[derive(sqlx::FromRow, Debug)]
pub struct Config {
    pub name: String,
    pub value: String,
}

#[derive(sqlx::FromRow, Debug)]
pub struct Judgement {
    pub submission_id: i64,
    pub challenge_name: String,
    pub points: i32,
    pub valid: bool,
}

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct TeamScore {
    pub team: String,
    pub score: i64,
}
