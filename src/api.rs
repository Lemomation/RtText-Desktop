#![allow(dead_code)]

use crate::config::CONFIG;
use crate::models::*;
use chrono::{DateTime, Local, Utc};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct SupabaseClient {
    http: reqwest::Client,
    access_token: Arc<RwLock<Option<String>>>,
    refresh_token: Arc<RwLock<Option<String>>>,
    user_id: Arc<RwLock<Option<String>>>,
    bots_cache: Arc<RwLock<HashMap<String, DbBot>>>,
    people_cache: Arc<RwLock<HashMap<String, DbPerson>>>,
    image_cache_dir: PathBuf,
}

impl SupabaseClient {
    pub fn new() -> Self {
        let cache_dir = directories::ProjectDirs::from("com", "Lemomation", "RtText")
            .map(|dirs| dirs.cache_dir().join("images"))
            .unwrap_or_else(|| std::env::temp_dir().join("rttext_cache"));
        let _ = std::fs::create_dir_all(&cache_dir);

        Self {
            http: reqwest::Client::new(),
            access_token: Arc::new(RwLock::new(None)),
            refresh_token: Arc::new(RwLock::new(None)),
            user_id: Arc::new(RwLock::new(None)),
            bots_cache: Arc::new(RwLock::new(HashMap::new())),
            people_cache: Arc::new(RwLock::new(HashMap::new())),
            image_cache_dir: cache_dir,
        }
    }

    fn default_headers(&self, token: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("apikey", HeaderValue::from_static(CONFIG.supabase_anon_key));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if let Some(t) = token {
            if let Ok(val) = HeaderValue::from_str(&format!("Bearer {}", t)) {
                headers.insert(AUTHORIZATION, val);
            }
        }
        headers
    }

    pub async fn login(
        &self,
        email: &str,
        pass: &str,
    ) -> Result<SessionData, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/auth/v1/token?grant_type=password", CONFIG.supabase_url);
        let body = serde_json::json!({
            "email": email.trim(),
            "password": pass
        });

        let resp = self
            .http
            .post(&url)
            .headers(self.default_headers(None))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let err_text = resp.text().await.unwrap_or_default();
            return Err(format!("Login failed: {}", err_text).into());
        }

        let auth: AuthResponse = resp.json().await?;
        let session = SessionData {
            access_token: auth.access_token.clone(),
            refresh_token: auth.refresh_token.clone(),
            user_id: auth.user.id.clone(),
            email: auth.user.email.clone(),
        };

        *self.access_token.write().await = Some(session.access_token.clone());
        *self.refresh_token.write().await = session.refresh_token.clone();
        *self.user_id.write().await = Some(session.user_id.clone());

        println!("Successfully authenticated as: {}", session.email.as_deref().unwrap_or_default());
        Ok(session)
    }

    pub async fn signup(
        &self,
        email: &str,
        pass: &str,
        username: &str,
    ) -> Result<SessionData, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/auth/v1/signup", CONFIG.supabase_url);
        let body = serde_json::json!({
            "email": email.trim(),
            "password": pass,
            "data": {
                "username": username.trim()
            }
        });

        let resp = self
            .http
            .post(&url)
            .headers(self.default_headers(None))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let err_text = resp.text().await.unwrap_or_default();
            return Err(format!("Sign up failed: {}", err_text).into());
        }

        let auth: AuthResponse = resp.json().await?;
        let session = SessionData {
            access_token: auth.access_token.clone(),
            refresh_token: auth.refresh_token.clone(),
            user_id: auth.user.id.clone(),
            email: auth.user.email.clone(),
        };

        *self.access_token.write().await = Some(session.access_token.clone());
        *self.refresh_token.write().await = session.refresh_token.clone();
        *self.user_id.write().await = Some(session.user_id.clone());

        Ok(session)
    }

    pub async fn set_session(&self, session: &SessionData) {
        *self.access_token.write().await = Some(session.access_token.clone());
        *self.refresh_token.write().await = session.refresh_token.clone();
        *self.user_id.write().await = Some(session.user_id.clone());
    }

    pub async fn clear_session(&self) {
        *self.access_token.write().await = None;
        *self.refresh_token.write().await = None;
        *self.user_id.write().await = None;
        self.bots_cache.write().await.clear();
        self.people_cache.write().await.clear();
    }

    pub async fn get_token(&self) -> Option<String> {
        self.access_token.read().await.clone()
    }

    pub async fn get_user_id(&self) -> Option<String> {
        self.user_id.read().await.clone()
    }

    pub async fn fetch_profile(&self, user_id: &str) -> Result<DbProfile, Box<dyn std::error::Error + Send + Sync>> {
        let token = self.get_token().await;
        let url = format!(
            "{}/rest/v1/profiles?id=eq.{}&select=*&limit=1",
            CONFIG.supabase_url, user_id
        );

        let resp = self
            .http
            .get(&url)
            .headers(self.default_headers(token.as_deref()))
            .send()
            .await?;

        let profiles: Vec<DbProfile> = resp.json().await?;
        profiles
            .into_iter()
            .next()
            .ok_or_else(|| "Profile not found".into())
    }

    pub async fn fetch_people(&self) -> Result<Vec<DbPerson>, Box<dyn std::error::Error + Send + Sync>> {
        let token = self.get_token().await;
        let url = format!("{}/rest/v1/people?select=*&order=username.asc", CONFIG.supabase_url);

        let resp = self
            .http
            .get(&url)
            .headers(self.default_headers(token.as_deref()))
            .send()
            .await?;

        let people: Vec<DbPerson> = resp.json().await?;

        let mut cache = self.people_cache.write().await;
        for p in &people {
            cache.insert(p.id.clone(), p.clone());
        }

        Ok(people)
    }

    pub async fn fetch_bots(&self) -> Result<Vec<DbBot>, Box<dyn std::error::Error + Send + Sync>> {
        let token = self.get_token().await;
        let url = format!("{}/rest/v1/public_bots?select=*&order=created_at.asc", CONFIG.supabase_url);

        let resp = self
            .http
            .get(&url)
            .headers(self.default_headers(token.as_deref()))
            .send()
            .await?;

        let bots: Vec<DbBot> = resp.json().await?;

        let mut cache = self.bots_cache.write().await;
        for b in &bots {
            cache.insert(b.id.clone(), b.clone());
        }

        Ok(bots)
    }

    pub async fn fetch_conversations(
        &self,
        user_id: &str,
    ) -> Result<Vec<EnrichedConversation>, Box<dyn std::error::Error + Send + Sync>> {
        let token = self.get_token().await;

        // Ensure people and bots caches are populated
        if self.people_cache.read().await.is_empty() {
            let _ = self.fetch_people().await;
        }
        if self.bots_cache.read().await.is_empty() {
            let _ = self.fetch_bots().await;
        }

        let url = format!(
            "{}/rest/v1/conversations?or=(user_id.eq.{},dm_user_id.eq.{},is_group.eq.true)&select=*&order=last_message_at.desc",
            CONFIG.supabase_url, user_id, user_id
        );

        let resp = self
            .http
            .get(&url)
            .headers(self.default_headers(token.as_deref()))
            .send()
            .await?;

        let raw_convs: Vec<DbConversation> = resp.json().await?;

        // Fetch recent messages to populate last_message snippet
        let mut last_msgs: HashMap<String, String> = HashMap::new();
        let msg_url = format!(
            "{}/rest/v1/messages?select=conversation_id,content,role,media_url,created_at&order=created_at.desc&limit=120",
            CONFIG.supabase_url
        );
        if let Ok(m_resp) = self
            .http
            .get(&msg_url)
            .headers(self.default_headers(token.as_deref()))
            .send()
            .await
        {
            if let Ok(recent) = m_resp.json::<Vec<DbMessage>>().await {
                for m in recent {
                    if let Some(cid) = m.conversation_id {
                        if !last_msgs.contains_key(&cid) {
                            let text = m.content.unwrap_or_default();
                            let formatted = if m.media_url.is_some() && text.trim().is_empty() {
                                if m.role == "user" { "You: Photo".into() } else { "Photo".into() }
                            } else if m.role == "user" {
                                format!("You: {}", text)
                            } else {
                                text
                            };
                            last_msgs.insert(cid, formatted);
                        }
                    }
                }
            }
        }

        let bots_cache = self.bots_cache.read().await;
        let people_cache = self.people_cache.read().await;

        let mut enriched = Vec::new();
        for c in raw_convs {
            let is_bot = c.bot_id.is_some();
            let is_grp = c.is_group.unwrap_or(false);

            let (title, subtitle, avatar_letter, avatar_url, bubble_color) = if is_grp {
                let name = c.title.clone().unwrap_or_else(|| "Group Chat".into());
                let letter = name.chars().next().unwrap_or('G').to_uppercase().to_string();
                (name, "Group conversation".into(), letter, c.avatar_url.clone(), "#1E2530".into())
            } else if let Some(bot_id) = &c.bot_id {
                if let Some(bot) = bots_cache.get(bot_id) {
                    let letter = bot.name.chars().next().unwrap_or('B').to_uppercase().to_string();
                    let color = bot.bubble_color.clone().unwrap_or_else(|| "#FB7185".into());
                    (bot.name.clone(), bot.bio.clone().unwrap_or_default(), letter, bot.pfp_url.clone(), color)
                } else {
                    ("AI Character".into(), "Custom bot".into(), "B".into(), None, "#FB7185".into())
                }
            } else if let Some(dm_uid) = &c.dm_user_id {
                let peer_id = if dm_uid == user_id {
                    c.user_id.as_deref().unwrap_or_default()
                } else {
                    dm_uid.as_str()
                };
                if let Some(person) = people_cache.get(peer_id) {
                    let uname = person.username.clone().unwrap_or_else(|| "User".into());
                    let letter = uname.chars().next().unwrap_or('U').to_uppercase().to_string();
                    (uname, "Direct message".into(), letter, person.avatar_url.clone(), "#1E2530".into())
                } else {
                    ("User".into(), "Direct message".into(), "U".into(), None, "#1E2530".into())
                }
            } else {
                ("Chat".into(), "".into(), "C".into(), None, "#1E2530".into())
            };

            let timestamp = format_timestamp(c.last_message_at.as_deref().unwrap_or(""));
            let snippet = last_msgs.remove(&c.id).unwrap_or_default();

            enriched.push(EnrichedConversation {
                id: c.id,
                title,
                subtitle,
                avatar_letter,
                avatar_url,
                last_message: snippet,
                timestamp,
                is_bot,
                is_online: false,
                bubble_color,
                bot_id: c.bot_id,
                dm_user_id: c.dm_user_id,
            });
        }

        Ok(enriched)
    }

    pub async fn fetch_messages(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<DbMessage>, Box<dyn std::error::Error + Send + Sync>> {
        let token = self.get_token().await;
        let url = format!(
            "{}/rest/v1/messages?conversation_id=eq.{}&select=*&order=created_at.asc",
            CONFIG.supabase_url, conversation_id
        );

        let resp = self
            .http
            .get(&url)
            .headers(self.default_headers(token.as_deref()))
            .send()
            .await?;

        let msgs: Vec<DbMessage> = resp.json().await?;
        Ok(msgs)
    }

    pub async fn send_message(
        &self,
        conversation_id: &str,
        content: &str,
        sender_id: Option<&str>,
        media_url: Option<&str>,
    ) -> Result<DbMessage, Box<dyn std::error::Error + Send + Sync>> {
        let token = self.get_token().await;
        let url = format!("{}/rest/v1/messages", CONFIG.supabase_url);

        let mut body = serde_json::json!({
            "conversation_id": conversation_id,
            "role": "user",
            "content": content
        });

        if let Some(sid) = sender_id {
            body["sender_id"] = serde_json::json!(sid);
        }
        if let Some(murl) = media_url {
            body["media_url"] = serde_json::json!(murl);
        }

        let mut headers = self.default_headers(token.as_deref());
        headers.insert("Prefer", HeaderValue::from_static("return=representation"));

        let resp = self
            .http
            .post(&url)
            .headers(headers)
            .json(&body)
            .send()
            .await?;

        let created: Vec<DbMessage> = resp.json().await?;
        created
            .into_iter()
            .next()
            .ok_or_else(|| "Failed to insert message".into())
    }

    pub async fn upload_chat_image(
        &self,
        conversation_id: &str,
        bytes: Vec<u8>,
        extension: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let token = self.get_token().await;
        let file_id = format!("{:x}", rand_u128());
        let file_name = format!("{}.{}", file_id, extension);
        let path = format!("{}/{}", conversation_id, file_name);
        let upload_url = format!(
            "{}/storage/v1/object/chat_media/{}",
            CONFIG.supabase_url, path
        );

        let mime = match extension {
            "png" => "image/png",
            "webp" => "image/webp",
            _ => "image/jpeg",
        };

        let mut headers = HeaderMap::new();
        headers.insert("apikey", HeaderValue::from_static(CONFIG.supabase_anon_key));
        headers.insert(CONTENT_TYPE, HeaderValue::from_str(mime)?);
        if let Some(t) = token {
            headers.insert(AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {}", t))?);
        }

        let resp = self
            .http
            .post(&upload_url)
            .headers(headers)
            .body(bytes)
            .send()
            .await?;

        if !resp.status().is_success() {
            let err = resp.text().await.unwrap_or_default();
            return Err(format!("Storage upload failed: {}", err).into());
        }

        let public_url = format!(
            "{}/storage/v1/object/public/chat_media/{}",
            CONFIG.supabase_url, path
        );
        Ok(public_url)
    }

    pub async fn trigger_ai_reply(
        &self,
        conversation_id: &str,
        bot_id: Option<&str>,
    ) -> Result<AiReplyResponse, Box<dyn std::error::Error + Send + Sync>> {
        let token = self.get_token().await;
        let url = format!("{}/functions/v1/ai-reply", CONFIG.supabase_url);

        let mut body = serde_json::json!({
            "conversation_id": conversation_id
        });
        if let Some(bid) = bot_id {
            body["bot_id"] = serde_json::json!(bid);
        }

        let resp = self
            .http
            .post(&url)
            .headers(self.default_headers(token.as_deref()))
            .json(&body)
            .send()
            .await?;

        let reply: AiReplyResponse = resp.json().await?;
        Ok(reply)
    }

    pub async fn get_or_create_bot_conversation(
        &self,
        user_id: &str,
        bot_id: &str,
    ) -> Result<DbConversation, Box<dyn std::error::Error + Send + Sync>> {
        let token = self.get_token().await;
        let check_url = format!(
            "{}/rest/v1/conversations?user_id=eq.{}&bot_id=eq.{}&select=*&limit=1",
            CONFIG.supabase_url, user_id, bot_id
        );
        let resp = self
            .http
            .get(&check_url)
            .headers(self.default_headers(token.as_deref()))
            .send()
            .await?;
        let convs: Vec<DbConversation> = resp.json().await?;
        if let Some(c) = convs.into_iter().next() {
            return Ok(c);
        }

        let insert_url = format!("{}/rest/v1/conversations", CONFIG.supabase_url);
        let now = Utc::now().to_rfc3339();
        let body = serde_json::json!({
            "user_id": user_id,
            "bot_id": bot_id,
            "is_group": false,
            "last_message_at": now,
            "user_last_read_at": now,
            "dm_user_last_read_at": now
        });
        let mut headers = self.default_headers(token.as_deref());
        headers.insert("Prefer", HeaderValue::from_static("return=representation"));
        let create_resp = self
            .http
            .post(&insert_url)
            .headers(headers)
            .json(&body)
            .send()
            .await?;
        let created: Vec<DbConversation> = create_resp.json().await?;
        created
            .into_iter()
            .next()
            .ok_or_else(|| "Failed to create bot conversation".into())
    }

    pub async fn get_or_create_dm_conversation(
        &self,
        my_id: &str,
        peer_id: &str,
    ) -> Result<DbConversation, Box<dyn std::error::Error + Send + Sync>> {
        let token = self.get_token().await;
        let check_url = format!(
            "{}/rest/v1/conversations?or=(and(user_id.eq.{},dm_user_id.eq.{}),and(user_id.eq.{},dm_user_id.eq.{}))&select=*&limit=1",
            CONFIG.supabase_url, my_id, peer_id, peer_id, my_id
        );
        let resp = self
            .http
            .get(&check_url)
            .headers(self.default_headers(token.as_deref()))
            .send()
            .await?;
        let convs: Vec<DbConversation> = resp.json().await?;
        if let Some(c) = convs.into_iter().next() {
            return Ok(c);
        }

        let insert_url = format!("{}/rest/v1/conversations", CONFIG.supabase_url);
        let now = Utc::now().to_rfc3339();
        let body = serde_json::json!({
            "user_id": my_id,
            "dm_user_id": peer_id,
            "is_group": false,
            "last_message_at": now,
            "user_last_read_at": now,
            "dm_user_last_read_at": now
        });
        let mut headers = self.default_headers(token.as_deref());
        headers.insert("Prefer", HeaderValue::from_static("return=representation"));
        let create_resp = self
            .http
            .post(&insert_url)
            .headers(headers)
            .json(&body)
            .send()
            .await?;
        let created: Vec<DbConversation> = create_resp.json().await?;
        created
            .into_iter()
            .next()
            .ok_or_else(|| "Failed to create DM conversation".into())
    }

    pub async fn check_latest_github_release(&self) -> Result<GithubRelease, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("https://api.github.com/repos/{}/releases/latest", CONFIG.github_repo);
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("RtText-Desktop"));

        let resp = self.http.get(&url).headers(headers).send().await?;
        if !resp.status().is_success() {
            return Err("Failed to query GitHub releases".into());
        }
        let rel: GithubRelease = resp.json().await?;
        Ok(rel)
    }

    pub async fn fetch_and_cache_image(&self, url: &str) -> Option<Vec<u8>> {
        if url.is_empty() {
            return None;
        }

        let mut hasher = DefaultHasher::new();
        url.hash(&mut hasher);
        let hash_val = hasher.finish();
        let ext = if url.ends_with(".png") { "png" }
                  else if url.ends_with(".webp") { "webp" }
                  else { "jpg" };
        let filename = format!("{:x}.{}", hash_val, ext);
        let file_path = self.image_cache_dir.join(filename);

        if file_path.exists() {
            if let Ok(bytes) = std::fs::read(&file_path) {
                return Some(bytes);
            }
        }

        if let Ok(resp) = self.http.get(url).send().await {
            if resp.status().is_success() {
                if let Ok(bytes) = resp.bytes().await {
                    let vec_bytes = bytes.to_vec();
                    let _ = std::fs::write(&file_path, &vec_bytes);
                    return Some(vec_bytes);
                }
            }
        }
        None
    }
}

pub fn decode_slint_image(bytes: &[u8]) -> Option<slint::Image> {
    let dyn_img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (width, height) = dyn_img.dimensions();
    let mut buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(width, height);
    buffer.make_mut_bytes().copy_from_slice(dyn_img.as_raw());
    Some(slint::Image::from_rgba8(buffer))
}

pub fn decode_image_rgba(bytes: &[u8]) -> Option<DecodedImage> {
    let dyn_img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (width, height) = dyn_img.dimensions();
    Some(DecodedImage {
        width,
        height,
        rgba: dyn_img.into_raw(),
    })
}

fn rand_u128() -> u128 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let ptr = &now as *const _ as usize as u128;
    now ^ (ptr << 32)
}

pub fn format_timestamp(iso: &str) -> String {
    if iso.is_empty() {
        return "".into();
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(iso) {
        let local_dt = dt.with_timezone(&Local);
        let now = Local::now();
        if local_dt.date_naive() == now.date_naive() {
            local_dt.format("%H:%M").to_string()
        } else {
            local_dt.format("%b %d").to_string()
        }
    } else if let Ok(dt) = DateTime::parse_from_str(iso, "%Y-%m-%d %H:%M:%S%#z") {
        let local_dt = dt.with_timezone(&Local);
        local_dt.format("%H:%M").to_string()
    } else {
        iso.split('T')
            .nth(1)
            .and_then(|t| t.split(':').take(2).collect::<Vec<_>>().join(":").into())
            .unwrap_or_else(|| iso.to_string())
    }
}

pub fn format_bubble_time(iso: &str) -> String {
    if iso.is_empty() {
        return "".into();
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(iso) {
        let local_dt = dt.with_timezone(&Local);
        let s = local_dt.format("%I:%M %p").to_string();
        if let Some(stripped) = s.strip_prefix('0') {
            stripped.to_string()
        } else {
            s
        }
    } else {
        format_timestamp(iso)
    }
}

pub fn parse_hex_color(hex: &str) -> slint::Color {
    let clean = hex.trim().trim_start_matches('#');
    if clean.len() == 6 {
        if let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&clean[0..2], 16),
            u8::from_str_radix(&clean[2..4], 16),
            u8::from_str_radix(&clean[4..6], 16),
        ) {
            return slint::Color::from_argb_u8(255, r, g, b);
        }
    }
    slint::Color::from_argb_u8(255, 30, 37, 48) // Default dark grey
}
