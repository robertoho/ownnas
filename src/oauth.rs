//! Signing MCP clients in with OAuth 2.1 (the MCP authorization spec): Claude and other clients
//! register themselves (RFC 7591), send the person to OwnNAS to approve them, and trade the code
//! they get back for a token (with PKCE, S256 only). Approving makes an ordinary agent key
//! (`agent_keys.rs`): its secret is the access token, and refreshing turns the secret over. So
//! everything keys have (folders, level, approval, the log, revoking) applies to OAuth clients.

use crate::agent_keys;
use crate::auth;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// How long a code from the approval page can be traded for a token.
const CODE_SECONDS: i64 = 600;
const MOST_CLIENTS: i64 = 500;

pub const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS oauth_clients (
        client_id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        redirect_uris TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS oauth_codes (
        code_hash TEXT PRIMARY KEY,
        client_id TEXT NOT NULL,
        redirect_uri TEXT NOT NULL,
        challenge TEXT NOT NULL,
        key_id INTEGER NOT NULL,
        token TEXT NOT NULL,
        expires_at INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS oauth_refresh (
        token_hash TEXT PRIMARY KEY,
        key_id INTEGER NOT NULL,
        client_id TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
";

#[derive(Debug, Deserialize)]
pub struct Registration {
    #[serde(default)]
    pub redirect_uris: Vec<String>,
    #[serde(default)]
    pub client_name: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct Client {
    pub client_id: String,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
}

/// Where a client may send the person back: https, http on this computer only (desktop
/// tools), or an app's own address ("cursor://…"); never scripts, data or files.
pub fn redirect_allowed(uri: &str) -> bool {
    if uri.contains('#') || uri.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let Some((scheme, rest)) = uri.split_once("://") else { return false };
    let scheme = scheme.to_ascii_lowercase();
    if scheme.is_empty() || !scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        return false;
    }
    let authority = rest.split(['/', '?']).next().unwrap_or_default();
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = if host.starts_with('[') { host.split(']').next().map(|inner| format!("{inner}]")).unwrap_or_default() } else { host.split(':').next().unwrap_or_default().to_string() };
    match scheme.as_str() {
        "https" => !host.is_empty(),
        "http" => matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]"),
        "javascript" | "data" | "file" | "vbscript" | "blob" => false,
        _ => true,
    }
}

pub fn register(conn: &Connection, registration: &Registration, now: i64) -> Result<Client, String> {
    if registration.redirect_uris.is_empty() || registration.redirect_uris.len() > 10 {
        return Err("Give between 1 and 10 redirect_uris".into());
    }
    if let Some(bad) = registration.redirect_uris.iter().find(|uri| !redirect_allowed(uri)) {
        return Err(format!("That redirect_uri is not allowed: {bad}"));
    }
    let count: i64 = conn.query_row("SELECT count(*) FROM oauth_clients", [], |row| row.get(0)).unwrap_or(0);
    if count >= MOST_CLIENTS {
        // The oldest go: registering needs no account, so it must not fill the database.
        let _ = conn.execute("DELETE FROM oauth_clients WHERE client_id IN (SELECT client_id FROM oauth_clients ORDER BY created_at LIMIT 50)", []);
    }
    let name: String = registration.client_name.clone().unwrap_or_else(|| "MCP client".into()).chars().filter(|c| !c.is_control()).take(80).collect();
    let name = if name.trim().is_empty() { "MCP client".to_string() } else { name.trim().to_string() };
    let client = Client { client_id: format!("ownnas_client_{}", &auth::new_token()[..24]), client_name: name, redirect_uris: registration.redirect_uris.clone() };
    conn.execute(
        "INSERT INTO oauth_clients(client_id, name, redirect_uris, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![client.client_id, client.client_name, serde_json::to_string(&client.redirect_uris).unwrap_or_default(), now],
    )
    .map_err(|_| "Could not register the client".to_string())?;
    Ok(client)
}

pub fn client(conn: &Connection, client_id: &str) -> Option<Client> {
    conn.query_row("SELECT client_id, name, redirect_uris FROM oauth_clients WHERE client_id = ?1", [client_id], |row| {
        Ok(Client { client_id: row.get(0)?, client_name: row.get(1)?, redirect_uris: serde_json::from_str(&row.get::<_, String>(2)?).unwrap_or_default() })
    })
    .optional()
    .ok()
    .flatten()
}

/// What an approval request asks, checked: the client, where to go back, and the PKCE challenge.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Request {
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    #[serde(default)]
    pub code_challenge_method: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub response_type: String,
    #[serde(default)]
    pub scope: String,
}

pub fn check_request(conn: &Connection, request: &Request) -> Result<Client, String> {
    let client = client(conn, &request.client_id).ok_or("That client is not registered (or was registered too long ago): add it again")?;
    if !client.redirect_uris.contains(&request.redirect_uri) {
        return Err("That redirect_uri was not registered for the client".into());
    }
    if request.response_type != "code" {
        return Err("Only response_type=code is supported".into());
    }
    if request.code_challenge_method != "S256" || request.code_challenge.len() < 43 {
        return Err("PKCE with code_challenge_method=S256 is required".into());
    }
    Ok(client)
}

/// After the person approved: the key made for the client, and a code to trade for its token.
pub fn approve(conn: &Connection, request: &Request, key_id: i64, token: &str, now: i64) -> Result<String, String> {
    let code = auth::new_token();
    // The key's secret waits with the code, to be handed out once.
    conn.execute(
        "INSERT INTO oauth_codes(code_hash, client_id, redirect_uri, challenge, key_id, token, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![auth::token_hash(&code), request.client_id, request.redirect_uri, request.code_challenge, key_id, token, now + CODE_SECONDS],
    )
    .map_err(|_| "Could not store the approval".to_string())?;
    let _ = conn.execute("DELETE FROM oauth_codes WHERE expires_at < ?1", [now]);
    Ok(code)
}

#[derive(Debug, Serialize)]
pub struct Tokens {
    pub access_token: String,
    pub token_type: &'static str,
    pub refresh_token: String,
    pub scope: String,
}

/// base64url without padding (RFC 4648 §5), as PKCE uses.
fn base64url(bytes: &[u8]) -> String {
    const LETTERS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 2);
    for group in bytes.chunks(3) {
        let joined = (u32::from(group[0]) << 16) | (u32::from(*group.get(1).unwrap_or(&0)) << 8) | u32::from(*group.get(2).unwrap_or(&0));
        out.push(LETTERS[(joined >> 18) as usize & 63] as char);
        out.push(LETTERS[(joined >> 12) as usize & 63] as char);
        if group.len() > 1 {
            out.push(LETTERS[(joined >> 6) as usize & 63] as char);
        }
        if group.len() > 2 {
            out.push(LETTERS[joined as usize & 63] as char);
        }
    }
    out
}

fn challenge_of(verifier: &str) -> String {
    base64url(&Sha256::digest(verifier.as_bytes()))
}

fn new_refresh(conn: &Connection, key_id: i64, client_id: &str, now: i64) -> Result<String, String> {
    let refresh = format!("ownnas_refresh_{}", auth::new_token());
    conn.execute("INSERT INTO oauth_refresh(token_hash, key_id, client_id, created_at) VALUES (?1, ?2, ?3, ?4)", params![auth::token_hash(&refresh), key_id, client_id, now])
        .map_err(|_| "Could not store the token".to_string())?;
    Ok(refresh)
}

/// grant_type=authorization_code: the code (once), checked against the client, where it went
/// back, and the PKCE verifier.
pub fn exchange(conn: &Connection, code: &str, client_id: &str, redirect_uri: &str, verifier: &str, now: i64) -> Result<Tokens, &'static str> {
    let found: Option<(String, String, String, i64, String, i64)> = conn
        .query_row(
            "SELECT client_id, redirect_uri, challenge, key_id, token, expires_at FROM oauth_codes WHERE code_hash = ?1",
            [auth::token_hash(code)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        )
        .optional()
        .map_err(|_| "invalid_grant")?;
    // Used once, whatever happens next.
    let _ = conn.execute("DELETE FROM oauth_codes WHERE code_hash = ?1", [auth::token_hash(code)]);
    let Some((for_client, for_redirect, challenge, key_id, token, expires_at)) = found else { return Err("invalid_grant") };
    if for_client != client_id || for_redirect != redirect_uri || expires_at < now || challenge_of(verifier) != challenge {
        return Err("invalid_grant");
    }
    if agent_keys::usable(conn, key_id, now).ok().flatten().is_none() {
        return Err("invalid_grant");
    }
    let refresh = new_refresh(conn, key_id, client_id, now).map_err(|_| "server_error")?;
    Ok(Tokens { access_token: token, token_type: "Bearer", refresh_token: refresh, scope: "mcp".into() })
}

/// grant_type=refresh_token: a new access token (the key's secret is turned over, so the old
/// one stops working) and a new refresh token.
pub fn refresh(conn: &mut Connection, refresh_token: &str, client_id: &str, now: i64) -> Result<Tokens, &'static str> {
    let found: Option<(i64, String)> = conn
        .query_row("SELECT key_id, client_id FROM oauth_refresh WHERE token_hash = ?1", [auth::token_hash(refresh_token)], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()
        .map_err(|_| "invalid_grant")?;
    let Some((key_id, for_client)) = found else { return Err("invalid_grant") };
    if for_client != client_id {
        return Err("invalid_grant");
    }
    let _ = conn.execute("DELETE FROM oauth_refresh WHERE token_hash = ?1", [auth::token_hash(refresh_token)]);
    if agent_keys::usable(conn, key_id, now).ok().flatten().is_none() {
        return Err("invalid_grant");
    }
    let token = agent_keys::new_secret(conn, key_id).map_err(|_| "server_error")?;
    let refresh = new_refresh(conn, key_id, client_id, now).map_err(|_| "server_error")?;
    Ok(Tokens { access_token: token, token_type: "Bearer", refresh_token: refresh, scope: "mcp".into() })
}

/// The key an approval made, for the log ("Claude (connected with OAuth)").
pub fn key_name(client: &Client) -> String {
    format!("{} (connected)", client.client_name)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirects_must_be_safe() {
        assert!(redirect_allowed("https://claude.ai/api/mcp/auth_callback"));
        assert!(redirect_allowed("http://localhost:33418/callback"));
        assert!(redirect_allowed("http://127.0.0.1:5000/cb"));
        assert!(!redirect_allowed("http://evil.example/cb"), "plain http elsewhere");
        assert!(!redirect_allowed("javascript:alert(1)"));
        assert!(!redirect_allowed("https://claude.ai/cb#fragment"));
        assert!(!redirect_allowed("not a url"));
        assert!(redirect_allowed("cursor://anysphere.cursor-retrieval/oauth/callback"));
        assert!(!redirect_allowed("data://text/html,x"));
    }

    #[test]
    fn pkce_challenge() {
        // RFC 7636, appendix B.
        assert_eq!(challenge_of("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }
}
