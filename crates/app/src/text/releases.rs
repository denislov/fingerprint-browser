//! release messages.
//!
//! The sentences of fetching a published build, shared by the two downloads
//! that do it - a browser core and an Xray engine. Both are fetched by the same
//! code (see [`crate::releases`]), so what it says when the request fails, when
//! a file cannot be written, when an archive will not read and when a second
//! download is asked for while one is running is one set of sentences rather
//! than two.
//!
//! What stays with each flow is what is not the machinery's to say: which list
//! was being read ([`super::cores`], [`super::xray`]), what a finished download
//! becomes, and what was left inside an archive.
use super::*;

impl Text {
    /// How far a download has got, as two sizes the caller has already written.
    pub fn release_progress(&self, received: &str, total: &str) -> String {
        match self.lang {
            Lang::En => format!("{received} of {total}"),
            Lang::Zh => format!("已下载 {received}，共 {total}"),
        }
    }

    /// The download itself did not finish.
    pub fn release_request_failed(&self, asset: &str, error: &str) -> String {
        match self.lang {
            Lang::En => format!("could not download {asset}: {error}"),
            Lang::Zh => format!("无法下载 {asset}：{error}"),
        }
    }

    /// The file could not be written, or moved into place.
    pub fn release_disk_failed(&self, path: &str, error: &str) -> String {
        match self.lang {
            Lang::En => format!("could not write {path}: {error}"),
            Lang::Zh => format!("无法写入 {path}：{error}"),
        }
    }

    /// The archive did not unpack.
    pub fn release_unpack_failed(&self, asset: &str, error: &str) -> String {
        match self.lang {
            Lang::En => format!("{asset} downloaded but did not unpack: {error}"),
            Lang::Zh => format!("{asset} 已下载，但解压失败：{error}"),
        }
    }

    /// Refusing a second download while one is running.
    pub fn release_busy(&self, asset: &str) -> String {
        match self.lang {
            Lang::En => format!("{asset} is already downloading; wait for it to finish"),
            Lang::Zh => format!("{asset} 正在下载中，请等待它完成"),
        }
    }
}
