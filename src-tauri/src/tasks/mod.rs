//! 常驻后台任务集合（按领域拆分）。

pub(crate) mod avatar;
pub(crate) mod entity_classify;
pub(crate) mod hourly_summary;
pub(crate) mod recording;
pub(crate) mod storage;

pub(crate) use avatar::*;
pub(crate) use entity_classify::*;
pub(crate) use hourly_summary::*;
pub(crate) use recording::*;
pub(crate) use storage::*;
