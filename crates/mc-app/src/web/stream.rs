//! 청취 시간이 아니라 실제 전송만 세고, 모든 길드의 바이트를 같은 회선 예산으로 보낸다.

use crate::media::cache::CachePin;
use crate::media::ytdlp::{DirectAudioSource, YtDlp};
use crate::models::TrackRef;
use crate::remote::models::WebStreamSettings;
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use std::time::SystemTime;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::time::Instant;

pub const BLOB_LIMIT: u64 = 32 * 1024 * 1024;
const RANGE_LIMIT: u64 = 1024 * 1024;
const CHUNK: usize = 16 * 1024;
const USER_MINUTE_BYTES: u64 = 64 * 1024 * 1024;
const QUEUE_LIMIT: usize = 512;
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Clone)]
struct Job {
    user: u64,
    prefetch: bool,
    deadline: Instant,
    active: bool,
}

#[derive(Default)]
struct Queue {
    serial: u64,
    jobs: HashMap<u64, Job>,
    usage: HashMap<u64, (Instant, u64)>,
    sent: u64,
}

pub struct StreamService {
    pub settings: RwLock<WebStreamSettings>,
    queue: Mutex<Queue>,
    pace: tokio::sync::Mutex<Instant>,
    etags: Mutex<HashMap<PathBuf, (u64, SystemTime, String)>>,
    origins: Mutex<
        HashMap<
            String,
            (
                Instant,
                Arc<tokio::sync::OnceCell<Option<DirectAudioSource>>>,
            ),
        >,
    >,
    origin_limit: tokio::sync::Semaphore,
}

pub struct Transfer {
    service: Arc<StreamService>,
    id: u64,
    pub expires: Instant,
}

impl Drop for Transfer {
    fn drop(&mut self) {
        self.service.queue.lock().unwrap().jobs.remove(&self.id);
    }
}

impl StreamService {
    pub fn new(mut settings: WebStreamSettings) -> Arc<Self> {
        settings.sanitize();
        Arc::new(Self {
            settings: RwLock::new(settings),
            queue: Mutex::new(Queue::default()),
            pace: tokio::sync::Mutex::new(Instant::now()),
            etags: Mutex::new(HashMap::new()),
            origins: Mutex::new(HashMap::new()),
            origin_limit: tokio::sync::Semaphore::new(2),
        })
    }

    pub fn status(&self) -> Value {
        let settings = self.settings.read().unwrap().clone();
        let queue = self.queue.lock().unwrap();
        json!({
            "enabled": settings.enabled, "maxTransfers": settings.max_transfers,
            "bandwidthKbps": settings.bandwidth_kbps, "blobLimitBytes": BLOB_LIMIT,
            "active": queue.jobs.values().filter(|job| job.active).count(),
            "queued": queue.jobs.values().filter(|job| !job.active).count(),
            "sentBytes": queue.sent,
            "routes": settings.routes, "prefetch": settings.prefetch,
        })
    }

    pub async fn origin_source(
        &self,
        track: &TrackRef,
        extractor: &YtDlp,
    ) -> Option<DirectAudioSource> {
        let cell = {
            let mut origins = self.origins.lock().unwrap();
            origins.retain(|_, (created, _)| created.elapsed() < Duration::from_secs(60));
            if origins.len() >= 128 && !origins.contains_key(&track.cache_key()) {
                return None;
            }
            origins
                .entry(track.cache_key())
                .or_insert_with(|| (Instant::now(), Arc::new(tokio::sync::OnceCell::new())))
                .1
                .clone()
        };
        cell.get_or_init(|| async {
            let _permit = self.origin_limit.try_acquire().ok()?;
            extractor.public_audio_source(track).await
        })
        .await
        .clone()
        .filter(|source| {
            source
                .expires_at
                .is_none_or(|expiry| expiry > chrono::Utc::now().timestamp() + 30)
        })
    }

    fn enqueue(
        self: &Arc<Self>,
        user: u64,
        prefetch: bool,
        deadline: Instant,
    ) -> Result<Transfer, StatusCode> {
        let mut queue = self.queue.lock().unwrap();
        if queue.jobs.len() >= QUEUE_LIMIT
            || queue.jobs.values().filter(|job| job.user == user).count() >= 2
        {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        queue.serial += 1;
        let id = queue.serial;
        queue.jobs.insert(
            id,
            Job {
                user,
                prefetch,
                deadline,
                active: false,
            },
        );
        Ok(Transfer {
            service: self.clone(),
            id,
            expires: Instant::now() + TRANSFER_TIMEOUT,
        })
    }

    fn admit(&self, id: u64) -> bool {
        let settings = self.settings.read().unwrap().clone();
        let mut queue = self.queue.lock().unwrap();
        let active = queue.jobs.values().filter(|job| job.active).count();
        if !settings.enabled || active >= settings.max_transfers as usize {
            return false;
        }
        let first = queue
            .jobs
            .iter()
            .filter(|(_, job)| !job.active)
            .min_by_key(|(serial, job)| (job.prefetch, job.deadline, **serial))
            .map(|(serial, _)| *serial);
        if first != Some(id) {
            return false;
        }
        let job = queue.jobs.get_mut(&id).unwrap();
        // 미리받기가 모든 자리를 채우면 새 청취자는 가장 느린 다운로드까지 기다리게 된다.
        if job.prefetch && active >= settings.max_transfers.saturating_sub(1).max(1) as usize {
            return false;
        }
        job.active = true;
        true
    }

    pub async fn acquire(
        self: &Arc<Self>,
        user: u64,
        prefetch: bool,
        deadline: Instant,
    ) -> Result<Transfer, StatusCode> {
        let transfer = self.enqueue(user, prefetch, deadline)?;
        let wait_until = (Instant::now() + Duration::from_secs(25)).min(deadline);
        loop {
            if !self.settings.read().unwrap().enabled {
                return Err(StatusCode::SERVICE_UNAVAILABLE);
            }
            if Instant::now() >= wait_until {
                return Err(StatusCode::SERVICE_UNAVAILABLE);
            }
            if self.admit(transfer.id) {
                return Ok(transfer);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn charge(&self, transfer: &Transfer, bytes: usize) -> std::io::Result<()> {
        // 잠금을 기다리는 시간까지 본문 수명에 넣어 느린 연결이 영구 슬롯이 되지 않게 한다.
        let mut pace = self.pace.lock().await;
        let settings = self.settings.read().unwrap().clone();
        if !settings.enabled
            || !settings.routes.iter().any(|route| {
                route.enabled && route.source == crate::remote::models::WebStreamSource::Server
            })
        {
            return Err(std::io::Error::other("stream disabled"));
        }
        {
            let mut queue = self.queue.lock().unwrap();
            let user = queue
                .jobs
                .get(&transfer.id)
                .ok_or_else(|| std::io::Error::other("transfer gone"))?
                .user;
            queue
                .usage
                .retain(|_, (since, _)| since.elapsed() < Duration::from_secs(60));
            let usage = queue.usage.entry(user).or_insert((Instant::now(), 0));
            if usage.1 + bytes as u64 > USER_MINUTE_BYTES {
                return Err(std::io::Error::other("user byte budget"));
            }
            usage.1 += bytes as u64;
            queue.sent += bytes as u64;
        }
        let interval =
            Duration::from_secs_f64(bytes as f64 * 8.0 / (settings.bandwidth_kbps as f64 * 1000.0));
        *pace = (*pace).max(Instant::now()) + interval;
        tokio::time::sleep_until(*pace).await;
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
enum RangeResult {
    Full,
    Partial(u64, u64),
    Unsatisfiable,
}

fn byte_range(value: Option<&str>, size: u64) -> RangeResult {
    let Some(value) = value else {
        return RangeResult::Full;
    };
    let Some(value) = value.strip_prefix("bytes=") else {
        return RangeResult::Full;
    };
    // multipart를 지원한다고 가장하지 않는다. HTTP는 Range 무시 후 200을 허용한다.
    if value.contains(',') {
        return RangeResult::Full;
    }
    let Some((first, last)) = value.split_once('-') else {
        return RangeResult::Full;
    };
    let number = |part: &str| {
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            None
        } else {
            part.parse::<u64>().ok()
        }
    };
    if first.is_empty() {
        let Some(suffix) = number(last) else {
            return RangeResult::Full;
        };
        if suffix == 0 || size == 0 {
            return RangeResult::Unsatisfiable;
        }
        let start = size.saturating_sub(suffix);
        return RangeResult::Partial(start, (size - 1).min(start.saturating_add(RANGE_LIMIT - 1)));
    }
    let Some(start) = number(first) else {
        return RangeResult::Full;
    };
    let end = if last.is_empty() {
        size.saturating_sub(1)
    } else if let Some(end) = number(last) {
        end
    } else {
        return RangeResult::Full;
    };
    if start >= size || start > end {
        return RangeResult::Unsatisfiable;
    }
    RangeResult::Partial(
        start,
        end.min(size - 1).min(start.saturating_add(RANGE_LIMIT - 1)),
    )
}

pub fn unavailable(status: StatusCode) -> Response {
    (
        status,
        [
            (header::CACHE_CONTROL, "private, no-store"),
            (header::RETRY_AFTER, "5"),
        ],
        "직접 받기를 기다리고 있어요.",
    )
        .into_response()
}

pub async fn file_response(
    mut file: tokio::fs::File,
    path: &Path,
    pin: CachePin,
    transfer: Transfer,
    headers: HeaderMap,
    method: Method,
) -> Response {
    let result = tokio::time::timeout_at(transfer.expires, async {
        let metadata = file.metadata().await?;
        let size = metadata.len();
        let modified = metadata.modified()?;
        if size > BLOB_LIMIT {
            if let Some((cached_size, cached_modified, etag)) =
                transfer.service.etags.lock().unwrap().get(path)
            {
                if *cached_size == size && *cached_modified == modified {
                    return Ok((size, etag.clone()));
                }
            }
        }
        let mut digest = Sha256::new();
        let mut buffer = vec![0; 64 * 1024];
        loop {
            let read = file.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            digest.update(&buffer[..read]);
        }
        let hex: String = digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let etag = format!("\"{hex}\"");
        let after = file.metadata().await?;
        if after.len() != size || after.modified()? != modified {
            return Err(std::io::Error::other("cache changed during hashing"));
        }
        if size > BLOB_LIMIT {
            let mut etags = transfer.service.etags.lock().unwrap();
            if etags.len() >= 128 {
                etags.clear();
            }
            etags.insert(path.to_owned(), (size, modified, etag.clone()));
        }
        Ok::<_, std::io::Error>((size, etag))
    })
    .await;
    let Ok(Ok((size, etag))) = result else {
        return unavailable(StatusCode::SERVICE_UNAVAILABLE);
    };
    let mut builder = Response::builder()
        .header(header::CACHE_CONTROL, "private, no-cache")
        .header(header::VARY, "Cookie")
        .header(header::ETAG, &etag)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_TYPE, "audio/ogg; codecs=opus")
        .header("X-Content-Type-Options", "nosniff");
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|tag| tag.trim().trim_start_matches("W/") == etag || tag.trim() == "*")
        })
    {
        return builder
            .status(StatusCode::NOT_MODIFIED)
            .body(Body::empty())
            .unwrap();
    }
    let range = if method == Method::HEAD
        || headers
            .get(header::IF_RANGE)
            .is_some_and(|value| value.to_str().ok() != Some(etag.as_str()))
    {
        RangeResult::Full
    } else {
        byte_range(
            headers
                .get(header::RANGE)
                .and_then(|value| value.to_str().ok()),
            size,
        )
    };
    let (start, length) = match range {
        RangeResult::Unsatisfiable => {
            return builder
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{size}"))
                .body(Body::empty())
                .unwrap();
        }
        RangeResult::Full => (0, size),
        RangeResult::Partial(start, end) => {
            builder = builder
                .status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"));
            (start, end - start + 1)
        }
    };
    builder = builder.header(header::CONTENT_LENGTH, length);
    if method == Method::HEAD {
        return builder.body(Body::empty()).unwrap();
    }
    if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        return unavailable(StatusCode::SERVICE_UNAVAILABLE);
    }
    let (sender, receiver) = tokio::sync::mpsc::channel::<std::io::Result<Bytes>>(1);
    tokio::spawn(async move {
        // 응답 소비자가 읽기를 멈춰도 이 태스크의 시한은 돈다. 본문 poll에만 시한을
        // 걸면 느린 연결이 다시 poll하지 않는 동안 파일과 슬롯을 영구히 붙들게 된다.
        let result = tokio::time::timeout_at(transfer.expires, async {
            let mut remaining = length;
            while remaining > 0 {
                let mut chunk = vec![0; remaining.min(CHUNK as u64) as usize];
                let read = file.read(&mut chunk).await?;
                if read == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "cache truncated",
                    ));
                }
                chunk.truncate(read);
                transfer.service.charge(&transfer, read).await?;
                if sender.send(Ok(Bytes::from(chunk))).await.is_err() {
                    return Ok(());
                }
                remaining -= read as u64;
            }
            Ok::<_, std::io::Error>(())
        })
        .await
        .unwrap_or_else(|_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "transfer expired",
            ))
        });
        drop(file);
        drop(pin);
        drop(transfer);
        if let Err(error) = result {
            let _ = sender.try_send(Err(error));
        }
    });
    let body = futures_util::stream::unfold(receiver, |mut receiver| async {
        receiver.recv().await.map(|chunk| (chunk, receiver))
    });
    builder.body(Body::from_stream(body)).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_handle_edges_and_ignore_unsupported_forms() {
        assert_eq!(
            byte_range(Some("bytes=0-0"), 10),
            RangeResult::Partial(0, 0)
        );
        assert_eq!(byte_range(Some("bytes=4-"), 10), RangeResult::Partial(4, 9));
        assert_eq!(
            byte_range(Some("bytes=-40"), 10),
            RangeResult::Partial(0, 9)
        );
        assert_eq!(
            byte_range(Some("bytes=3-99"), 10),
            RangeResult::Partial(3, 9)
        );
        for value in ["bytes=-0", "bytes=10-", "bytes=7-2"] {
            assert_eq!(byte_range(Some(value), 10), RangeResult::Unsatisfiable);
        }
        for value in [
            "bytes=0-1,4-5",
            "bytes=+1-2",
            "bytes=wat",
            "items=0-1",
            "bytes=18446744073709551616-",
        ] {
            assert_eq!(byte_range(Some(value), 10), RangeResult::Full);
        }
        assert_eq!(byte_range(Some("bytes=0-"), 0), RangeResult::Unsatisfiable);
    }

    #[test]
    fn large_ranges_are_bounded_and_resume_at_the_reported_end() {
        assert_eq!(
            byte_range(Some("bytes=0-"), 3 * RANGE_LIMIT),
            RangeResult::Partial(0, RANGE_LIMIT - 1)
        );
        assert_eq!(
            byte_range(Some(&format!("bytes={RANGE_LIMIT}-")), 3 * RANGE_LIMIT),
            RangeResult::Partial(RANGE_LIMIT, 2 * RANGE_LIMIT - 1)
        );
        assert_eq!(
            byte_range(Some("bytes=7-9999999"), 3 * RANGE_LIMIT),
            RangeResult::Partial(7, RANGE_LIMIT + 6)
        );
        assert_eq!(
            byte_range(
                Some(&format!("bytes=-{}", 2 * RANGE_LIMIT)),
                3 * RANGE_LIMIT
            ),
            RangeResult::Partial(RANGE_LIMIT, 2 * RANGE_LIMIT - 1)
        );
        assert_eq!(byte_range(None, 3 * RANGE_LIMIT), RangeResult::Full);
    }

    #[tokio::test]
    async fn current_has_priority_and_cancelled_jobs_release_capacity() {
        let service = StreamService::new(WebStreamSettings {
            enabled: true,
            max_transfers: 2,
            ..Default::default()
        });
        let deadline = Instant::now() + Duration::from_secs(60);
        let later = service.enqueue(1, true, deadline).unwrap();
        let earlier = service
            .enqueue(2, true, deadline - Duration::from_secs(10))
            .unwrap();
        let current = service.enqueue(3, false, deadline).unwrap();
        assert!(!service.admit(later.id));
        assert!(service.admit(current.id));
        assert!(!service.admit(earlier.id));
        drop(current);
        assert!(service.admit(earlier.id));
        drop(earlier);
        assert!(service.admit(later.id));
        drop(later);
        assert_eq!(service.status()["active"], 0);
        assert_eq!(service.status()["queued"], 0);
    }

    #[tokio::test]
    async fn pacing_is_shared_by_all_users_and_settings_are_bounded() {
        let service = StreamService::new(WebStreamSettings {
            enabled: true,
            max_transfers: 99,
            bandwidth_kbps: 0,
            ..Default::default()
        });
        assert_eq!(service.settings.read().unwrap().max_transfers, 30);
        assert_eq!(service.settings.read().unwrap().bandwidth_kbps, 128);
        let deadline = Instant::now() + Duration::from_secs(60);
        let first = service.acquire(1, false, deadline).await.unwrap();
        let second = service.acquire(2, false, deadline).await.unwrap();
        let started = Instant::now();
        let (one, two) = tokio::join!(service.charge(&first, 1600), service.charge(&second, 1600));
        one.unwrap();
        two.unwrap();
        assert!(started.elapsed() >= Duration::from_millis(200));
        service.settings.write().unwrap().enabled = false;
        assert!(service.charge(&first, 1).await.is_err());
    }
}
