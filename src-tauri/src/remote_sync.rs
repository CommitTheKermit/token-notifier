use crate::parser::UsageSource;
use crate::settings::RemoteSyncSettings;
use crate::storage::{RemoteHourlyPoint, UsageStore};
use chrono::{DateTime, Duration, SecondsFormat, TimeZone, Utc};
use reqwest::Client;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use url::Url;

pub const OPENAI_PROVIDER: &str = "openai";
pub const ANTHROPIC_PROVIDER: &str = "anthropic";

const OPENAI_USAGE_URL: &str = "https://api.openai.com/v1/organization/usage/completions";
const ANTHROPIC_USAGE_URL: &str =
    "https://api.anthropic.com/v1/organizations/usage_report/messages";
const MAX_HOURLY_BUCKETS_PER_REQUEST: u64 = 168;
const MAX_PAGES: usize = 10;

pub async fn sync_once(
    db_path: PathBuf,
    settings: &RemoteSyncSettings,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    if !settings.enabled {
        return Ok(());
    }

    let client = Client::builder()
        .user_agent("TokenNotifier/0.1.0")
        .timeout(std::time::Duration::from_secs(20))
        .build()?;

    if settings.openai_enabled {
        sync_provider(
            &db_path,
            OPENAI_PROVIDER,
            fetch_openai_usage(&client, settings, now).await,
            now,
        )?;
    }

    if settings.anthropic_enabled {
        sync_provider(
            &db_path,
            ANTHROPIC_PROVIDER,
            fetch_anthropic_usage(&client, settings, now).await,
            now,
        )?;
    }

    Ok(())
}

fn sync_provider(
    db_path: &PathBuf,
    provider: &str,
    result: anyhow::Result<Vec<RemoteHourlyPoint>>,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    let mut store = UsageStore::open(db_path)?;
    match result {
        Ok(points) => {
            store.upsert_remote_hourly_points(provider, &points, now)?;
            store.mark_remote_sync_state(
                provider,
                "ok",
                Some(&format!("{} hourly buckets synced", points.len())),
                now,
            )?;
        }
        Err(error) => {
            store.mark_remote_sync_state(provider, "error", Some(&error.to_string()), now)?;
        }
    }
    Ok(())
}

async fn fetch_openai_usage(
    client: &Client,
    settings: &RemoteSyncSettings,
    now: DateTime<Utc>,
) -> anyhow::Result<Vec<RemoteHourlyPoint>> {
    let key = std::env::var("OPENAI_ADMIN_KEY")
        .or_else(|_| std::env::var("OPENAI_API_KEY"))
        .map_err(|_| anyhow::anyhow!("OPENAI_ADMIN_KEY is not set"))?;
    let start = now - Duration::hours(settings.lookback_hours as i64);
    let mut page: Option<String> = None;
    let mut points = Vec::new();

    for _ in 0..MAX_PAGES {
        let mut url = Url::parse(OPENAI_USAGE_URL)?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("start_time", &start.timestamp().to_string());
            query.append_pair("end_time", &now.timestamp().to_string());
            query.append_pair("bucket_width", "1h");
            query.append_pair("limit", &MAX_HOURLY_BUCKETS_PER_REQUEST.to_string());
            if let Some(page) = &page {
                query.append_pair("page", page);
            }
        }
        let value = client
            .get(url)
            .bearer_auth(&key)
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        points.extend(parse_openai_usage_points(&value));
        if value.get("has_more").and_then(Value::as_bool) != Some(true) {
            break;
        }
        page = value
            .get("next_page")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        if page.is_none() {
            break;
        }
    }

    Ok(aggregate_points(
        OPENAI_PROVIDER,
        UsageSource::Codex,
        points,
    ))
}

async fn fetch_anthropic_usage(
    client: &Client,
    settings: &RemoteSyncSettings,
    now: DateTime<Utc>,
) -> anyhow::Result<Vec<RemoteHourlyPoint>> {
    let key = std::env::var("ANTHROPIC_ADMIN_KEY")
        .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
        .map_err(|_| anyhow::anyhow!("ANTHROPIC_ADMIN_KEY is not set"))?;
    let start = now - Duration::hours(settings.lookback_hours as i64);
    let mut page: Option<String> = None;
    let mut points = Vec::new();

    for _ in 0..MAX_PAGES {
        let mut url = Url::parse(ANTHROPIC_USAGE_URL)?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("starting_at", &iso_seconds(start));
            query.append_pair("ending_at", &iso_seconds(now));
            query.append_pair("bucket_width", "1h");
            query.append_pair("limit", &MAX_HOURLY_BUCKETS_PER_REQUEST.to_string());
            if let Some(page) = &page {
                query.append_pair("page", page);
            }
        }
        let value = client
            .get(url)
            .header("anthropic-version", "2023-06-01")
            .header("x-api-key", &key)
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        points.extend(parse_anthropic_usage_points(&value));
        if value.get("has_more").and_then(Value::as_bool) != Some(true) {
            break;
        }
        page = value
            .get("next_page")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        if page.is_none() {
            break;
        }
    }

    Ok(aggregate_points(
        ANTHROPIC_PROVIDER,
        UsageSource::ClaudeCode,
        points,
    ))
}

fn parse_openai_usage_points(value: &Value) -> Vec<RemoteHourlyPoint> {
    parse_usage_points(
        value,
        OPENAI_PROVIDER,
        UsageSource::Codex,
        &["input_tokens", "output_tokens"],
    )
}

fn parse_anthropic_usage_points(value: &Value) -> Vec<RemoteHourlyPoint> {
    parse_usage_points(
        value,
        ANTHROPIC_PROVIDER,
        UsageSource::ClaudeCode,
        &[
            "input_tokens",
            "uncached_input_tokens",
            "cached_input_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
            "output_tokens",
        ],
    )
}

fn parse_usage_points(
    value: &Value,
    provider: &str,
    source: UsageSource,
    token_fields: &[&str],
) -> Vec<RemoteHourlyPoint> {
    value
        .get("data")
        .or_else(|| value.get("buckets"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|bucket| {
            let hour_start = bucket_start(bucket)?;
            let tokens = bucket
                .get("results")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|result| sum_token_fields(result, token_fields))
                .sum::<u64>();
            (tokens > 0).then(|| RemoteHourlyPoint {
                provider: provider.to_string(),
                source,
                hour_start,
                tokens_used: tokens,
            })
        })
        .collect()
}

fn bucket_start(bucket: &Value) -> Option<DateTime<Utc>> {
    if let Some(timestamp) = bucket.get("start_time").and_then(Value::as_i64) {
        return Utc.timestamp_opt(timestamp, 0).single();
    }
    bucket
        .get("starting_at")
        .or_else(|| bucket.get("start_at"))
        .and_then(Value::as_str)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
}

fn sum_token_fields(result: &Value, fields: &[&str]) -> u64 {
    let flat_tokens = fields
        .iter()
        .filter_map(|field| result.get(*field).and_then(Value::as_u64))
        .sum::<u64>();
    let cache_creation_tokens = result
        .get("cache_creation")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|fields| fields.values())
        .filter_map(Value::as_u64)
        .sum::<u64>();
    flat_tokens + cache_creation_tokens
}

fn aggregate_points(
    provider: &str,
    source: UsageSource,
    points: Vec<RemoteHourlyPoint>,
) -> Vec<RemoteHourlyPoint> {
    let mut by_hour = HashMap::<i64, u64>::new();
    for point in points {
        let hour = point.hour_start.timestamp() - point.hour_start.timestamp().rem_euclid(3600);
        *by_hour.entry(hour).or_default() += point.tokens_used;
    }
    let mut points = by_hour
        .into_iter()
        .filter_map(|(timestamp, tokens_used)| {
            Utc.timestamp_opt(timestamp, 0)
                .single()
                .map(|hour_start| RemoteHourlyPoint {
                    provider: provider.to_string(),
                    source,
                    hour_start,
                    tokens_used,
                })
        })
        .collect::<Vec<_>>();
    points.sort_by_key(|point| point.hour_start);
    points
}

fn iso_seconds(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_openai_hourly_usage_tokens() {
        let value = json!({
            "data": [{
                "start_time": 1779519600,
                "results": [{
                    "input_tokens": 100,
                    "output_tokens": 25,
                    "input_cached_tokens": 80
                }]
            }]
        });

        let points = parse_openai_usage_points(&value);
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].source, UsageSource::Codex);
        assert_eq!(points[0].tokens_used, 125);
    }

    #[test]
    fn parses_anthropic_hourly_usage_tokens() {
        let value = json!({
            "data": [{
                "starting_at": "2026-05-23T06:00:00Z",
                "results": [{
                    "uncached_input_tokens": 100,
                    "cache_read_input_tokens": 20,
                    "cache_creation": {
                        "ephemeral_1h_input_tokens": 30
                    },
                    "output_tokens": 50
                }]
            }]
        });

        let points = parse_anthropic_usage_points(&value);
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].source, UsageSource::ClaudeCode);
        assert_eq!(points[0].tokens_used, 200);
    }
}
