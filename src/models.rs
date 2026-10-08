#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AuthResponse {
    pub access_token: String,
    pub token_type: Option<String>,
    pub expires_in: Option<i64>,
    pub refresh_token: Option<String>,
    pub user: AuthUser,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AuthUser {
    pub id: String,
    pub email: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SessionData {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub user_id: String,
    pub email: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DbProfile {
    pub id: String,
    pub username: Option<String>,
    pub avatar_url: Option<String>,
    pub beads: Option<i32>,
    pub last_daily_claim: Option<String>,
    pub last_seen_at: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DbPerson {
    pub id: String,
    pub username: Option<String>,
    pub avatar_url: Option<String>,
    pub created_at: Option<String>,
    pub last_seen_at: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DbConversation {
    pub id: String,
    pub user_id: Option<String>,
    pub bot_id: Option<String>,
    pub dm_user_id: Option<String>,
    pub is_group: Option<bool>,
    pub title: Option<String>,
    pub avatar_url: Option<String>,
    pub last_message_at: Option<String>,
    pub created_at: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DbMessage {
    pub id: String,
    pub conversation_id: Option<String>,
    pub role: String,
    pub content: Option<String>,
    pub created_at: Option<String>,
    pub sender_id: Option<String>,
    pub bot_id: Option<String>,
    pub media_url: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DbBot {
    pub id: String,
    pub name: String,
    pub bio: Option<String>,
    pub description: Option<String>,
    pub pfp_url: Option<String>,
    pub bubble_color: Option<String>,
    pub is_public: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AiReplyResponse {
    pub content: Option<String>,
    pub beads: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct EnrichedConversation {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub avatar_letter: String,
    pub avatar_url: Option<String>,
    pub last_message: String,
    pub timestamp: String,
    pub is_bot: bool,
    pub is_online: bool,
    pub bubble_color: String,
    pub bot_id: Option<String>,
    pub dm_user_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GithubReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GithubRelease {
    pub tag_name: String,
    pub name: Option<String>,
    pub body: Option<String>,
    pub assets: Vec<GithubReleaseAsset>,
}
