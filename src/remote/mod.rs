//! 마참뮤직 사용자 포털의 도메인 계층.
//! 기존 관리자 웹과 분리된 길드별 점수 큐, 개인 음악, 채팅과 감사 로그를 담당한다.

pub mod models;
pub mod ranking;
pub mod store;

pub use models::*;
pub use store::RemoteStore;
