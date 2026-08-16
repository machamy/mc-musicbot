//! 마참뮤직 사용자 포털의 도메인 계층.
//! 기존 관리자 웹과 분리된 길드별 점수 큐, 개인 음악, 채팅과 감사 로그를 담당한다.
//!
//! 스키마는 `store::RemoteStore::open`의 `PRAGMA user_version` 러너가 단계별로 올린다.
//! 레거시(C# 공용) 테이블 — `settings` `playlists` `playlist_entries` `guild_states`
//! `guild_queue` `cache_entries` `blacklist` `guild_metadata` — 은 이 모듈이 절대 건드리지 않는다.
//! 새 테이블은 전부 `remote_` 접두사를 쓴다.
//!
//! 정렬 함수는 `ranking`에서 직접 가져다 쓴다:
//! `use crate::remote::ranking::{sort_queue, request_rounds, apply_rounds, wait_score_targets};`

pub mod models;
pub mod ranking;
pub mod store;
pub mod tj;

pub use models::*;
pub use store::RemoteStore;
