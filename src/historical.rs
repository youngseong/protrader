use std::sync::Arc;
use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use crate::auth::KisAuthProvider;

/// A single price observation replayed during backtesting.
#[derive(Debug, Clone)]
pub struct Tick {
    /// Wall-clock time in KST.
    pub time: NaiveDateTime,
    pub symbol: String,
    pub price: i64,
}

pub struct KisHistoricalClient {
    http: reqwest::Client,
    auth: Arc<KisAuthProvider>,
}

impl KisHistoricalClient {
    pub fn new(auth: Arc<KisAuthProvider>) -> Self {
        Self { http: reqwest::Client::new(), auth }
    }

    /// Fetch all minute-bar ticks for `symbol` on `date` (KST), sorted ascending by time.
    ///
    /// Reads from a local CSV cache at `data/YYYYMMDD/<symbol>.csv` when present so
    /// repeated backtest runs avoid redundant API calls. The cache is written on the
    /// first successful fetch.
    pub async fn fetch_day(&self, symbol: &str, date: NaiveDate) -> anyhow::Result<Vec<Tick>> {
        let cache = cache_path(symbol, date);
        if cache.exists() {
            return load_cache(&cache, symbol);
        }
        let ticks = self.fetch_from_api(symbol, date).await?;
        write_cache(&cache, &ticks)?;
        Ok(ticks)
    }

    async fn fetch_from_api(&self, symbol: &str, date: NaiveDate) -> anyhow::Result<Vec<Tick>> {
        let date_str = date.format("%Y%m%d").to_string();
        let open = NaiveTime::from_hms_opt(9, 0, 0).unwrap();

        let mut all: Vec<Tick> = Vec::new();
        // KIS returns ~30 bars per call in descending order; paginate backward
        // from the end of the session until we reach open.
        let mut query_time = NaiveTime::from_hms_opt(15, 30, 0).unwrap();

        loop {
            let batch = self
                .fetch_batch(symbol, &date_str, &query_time.format("%H%M%S").to_string())
                .await?;
            if batch.is_empty() {
                break;
            }
            // batch is descending; last element is the earliest tick in this page
            let earliest = batch.last().unwrap().time.time();
            all.extend(batch);
            if earliest <= open {
                break;
            }
            // Next call: query one minute before the earliest tick we already have
            query_time = earliest - chrono::Duration::minutes(1);
        }

        all.sort_by_key(|t| t.time);
        all.dedup_by_key(|t| t.time);
        all.retain(|t| t.time.time() >= open);
        Ok(all)
    }

    /// Single paginated call to KIS `inquire-time-itemchartprice` (tr_id FHKST03010200).
    /// Returns bars in the descending order KIS provides them.
    async fn fetch_batch(
        &self,
        symbol: &str,
        date: &str,
        hour: &str,
    ) -> anyhow::Result<Vec<Tick>> {
        #[derive(serde::Deserialize)]
        struct Bar {
            stck_cntg_hour: String,
            stck_prpr: String,
        }
        #[derive(serde::Deserialize)]
        struct Resp {
            output2: Option<Vec<Bar>>,
        }

        let token = self.auth.token().await;
        let resp: Resp = self
            .http
            .get(format!(
                "{}/uapi/domestic-stock/v1/quotations/inquire-time-itemchartprice",
                self.auth.base_url()
            ))
            .header("content-type", "application/json; charset=utf-8")
            .header("authorization", format!("Bearer {}", token))
            .header("appkey", self.auth.app_key())
            .header("appsecret", self.auth.app_secret())
            .header("tr_id", "FHKST03010200")
            .query(&[
                ("FID_ETC_CLS_CODE", ""),
                ("FID_COND_MRKT_DIV_CODE", "J"),
                ("FID_INPUT_ISCD", symbol),
                ("FID_INPUT_DATE_1", date),
                ("FID_INPUT_HOUR_1", hour),
                ("FID_PW_DATA_INCU_YN", "Y"),
            ])
            .send()
            .await?
            .json()
            .await?;

        let date_naive = NaiveDate::parse_from_str(date, "%Y%m%d")?;
        let bars = resp.output2.unwrap_or_default();

        let mut ticks = Vec::with_capacity(bars.len());
        for bar in bars {
            let price: i64 = match bar.stck_prpr.trim().parse() {
                Ok(p) if p > 0 => p,
                _ => continue,
            };
            let time = match NaiveTime::parse_from_str(bar.stck_cntg_hour.trim(), "%H%M%S") {
                Ok(t) => t,
                Err(_) => continue,
            };
            ticks.push(Tick {
                time: date_naive.and_time(time),
                symbol: symbol.to_string(),
                price,
            });
        }
        Ok(ticks)
    }
}

// ── CSV cache ─────────────────────────────────────────────────────────────────

fn cache_path(symbol: &str, date: NaiveDate) -> std::path::PathBuf {
    std::path::PathBuf::from("data")
        .join(date.format("%Y%m%d").to_string())
        .join(format!("{}.csv", symbol))
}

fn load_cache(path: &std::path::Path, symbol: &str) -> anyhow::Result<Vec<Tick>> {
    let content = std::fs::read_to_string(path)?;
    let mut ticks = Vec::new();
    for line in content.lines().skip(1) {
        let mut parts = line.splitn(3, ',');
        let (Some(ts), Some(_sym), Some(price_str)) =
            (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let time = NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%S")?;
        let price: i64 = price_str.trim().parse()?;
        ticks.push(Tick { time, symbol: symbol.to_string(), price });
    }
    Ok(ticks)
}

fn write_cache(path: &std::path::Path, ticks: &[Tick]) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut out = String::from("time,symbol,price\n");
    for t in ticks {
        out.push_str(&format!(
            "{},{},{}\n",
            t.time.format("%Y-%m-%dT%H:%M:%S"),
            t.symbol,
            t.price
        ));
    }
    std::fs::write(path, out)?;
    Ok(())
}

// ── Live smoke test ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Live API smoke test — requires KIS credentials in .env.
    /// Run with: cargo test live_historical -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn live_historical_fetch() {
        let _ = dotenvy::dotenv();
        let creds = crate::config::KisCredentials::from_env();
        let auth = crate::auth::KisAuthProvider::new(
            reqwest::Client::new(),
            "https://openapi.koreainvestment.com:9443".to_string(),
            creds,
        )
        .await
        .expect("auth failed");

        let client = KisHistoricalClient::new(auth);
        // Use a recent trading day (adjust as needed)
        let date = NaiveDate::from_ymd_opt(2026, 3, 26).unwrap();
        let ticks = client
            .fetch_day("005930", date)
            .await
            .expect("fetch failed");

        println!("Fetched {} ticks for 005930 on {}", ticks.len(), date);
        assert!(!ticks.is_empty());
        // Verify ordering
        for w in ticks.windows(2) {
            assert!(w[0].time <= w[1].time);
        }
    }
}
