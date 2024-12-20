use crate::state::{self, APP_USER_AGENT};
use anyhow::Context;
use redis::AsyncCommands;
use robotstxt::DefaultMatcher;
use utils::redis::Key;

pub async fn is_robots_allowed(url: &url::Url, state: &state::AppState) -> anyhow::Result<bool> {
    let domain = url
        .domain()
        .ok_or_else(|| anyhow::anyhow!("Missing domain for {}", url))?;

    let mut conn = state
        .redis_client
        .get_multiplexed_tokio_connection()
        .await
        .context("Failed to establish Redis connection")?;

    let key = Key::Robots(domain);
    if let Some(allowed) = conn
        .get::<_, Option<u8>>(&key)
        .await
        .context("Failed to GET from Redis")?
    {
        tracing::debug!(
            domain = domain,
            cached = true,
            "Robots.txt decision retrieved from cache"
        );
        return Ok(allowed == 1);
    }

    let scheme = url.scheme();
    let robots_url = format!("{}://{}/robots.txt", scheme, domain);
    let robots_url = url::Url::parse(&robots_url).context("Failed to parse robots.txt URL")?;

    tracing::debug!(domain = domain, url = %robots_url, "Fetching robots.txt");
    let content = state
        .reqwest_client
        .get(robots_url)
        .send()
        .await
        .context("Failed to fetch robots.txt")?
        .text()
        .await
        .context("Failed to read robots.txt content")?;

    let mut matcher = DefaultMatcher::default();
    let allowed = matcher.one_agent_allowed_by_robots(&content, APP_USER_AGENT, url.as_str());

    redis::pipe()
        .atomic()
        .set(&key, if allowed { 1 } else { 0 })
        .expire(&key, 60 * 60 * 24 * 30)
        .query_async::<()>(&mut conn)
        .await
        .context("Failed to SET & EXPIRE in Redis")?;

    tracing::debug!(
        domain = domain,
        allowed = allowed,
        "Robots.txt decision cached"
    );

    Ok(allowed)
}
