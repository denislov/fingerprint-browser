//! xray messages.
//!
//! The sentences for the one download this window makes that is not a browser
//! core. They live here rather than in [`super::cores`] because what goes wrong
//! is different: an engine archive is unpacked for two files rather than
//! searched for a browser, and what comes out of it becomes a setting instead of
//! a row.
use super::*;

impl Text {
    /// Why the list of published engines could not be read.
    pub fn xray_download_failed(&self, error: &str) -> String {
        match self.lang {
            Lang::En => format!("could not read the published Xray builds: {error}"),
            Lang::Zh => format!("无法读取已发布的 Xray 构建：{error}"),
        }
    }

    /// A download starting, named by the version and the architecture it is for.
    pub fn xray_release_downloading(&self, tag: &str, architecture: &str) -> String {
        match self.lang {
            Lang::En => format!("Downloading Xray {tag} for {architecture}"),
            Lang::Zh => format!("正在下载 {architecture} 版 Xray {tag}"),
        }
    }

    /// The download itself did not finish.
    pub fn xray_release_request_failed(&self, asset: &str, error: &str) -> String {
        match self.lang {
            Lang::En => format!("could not download {asset}: {error}"),
            Lang::Zh => format!("无法下载 {asset}：{error}"),
        }
    }

    /// The file could not be written, or moved into place.
    pub fn xray_release_disk_failed(&self, path: &str, error: &str) -> String {
        match self.lang {
            Lang::En => format!("could not write {path}: {error}"),
            Lang::Zh => format!("无法写入 {path}：{error}"),
        }
    }

    /// The archive did not unpack.
    pub fn xray_release_unpack_failed(&self, asset: &str, error: &str) -> String {
        match self.lang {
            Lang::En => format!("{asset} downloaded but did not unpack: {error}"),
            Lang::Zh => format!("{asset} 已下载，但解压失败：{error}"),
        }
    }

    /// The archive unpacked and no engine was found in it.
    ///
    /// The directory is named because the files that did arrive are still there,
    /// and the reader can point the setting at one by hand.
    pub fn xray_release_no_binary(&self, tag: &str, directory: &str) -> String {
        match self.lang {
            Lang::En => format!(
                "Xray {tag} was unpacked to {directory}, but no engine was found in it; point the Xray executable setting at one by hand"
            ),
            Lang::Zh => format!(
                "Xray {tag} 已解压到 {directory}，但没有找到可执行文件；请手动把 Xray 可执行文件设置指向它"
            ),
        }
    }

    /// Refusing a second download while one is running.
    pub fn xray_release_busy(&self, asset: &str) -> String {
        match self.lang {
            Lang::En => format!("{asset} is already downloading; wait for it to finish"),
            Lang::Zh => format!("{asset} 正在下载中，请等待它完成"),
        }
    }

    /// A downloaded engine that became the setting: where it is, and when it is
    /// used.
    ///
    /// The restart is named because this program reads the executable at the
    /// start of a run: a sentence that stopped at "downloaded" would leave the
    /// reader waiting for a profile to launch on the engine they just fetched.
    pub fn xray_release_saved(&self, tag: &str, path: &str) -> String {
        match self.lang {
            Lang::En => {
                format!("Xray {tag} is installed at {path} and will be used from the next start")
            }
            Lang::Zh => format!("Xray {tag} 已安装到 {path}，下次启动开始使用"),
        }
    }

    /// A downloaded engine that is on disk and not in force, because an
    /// environment variable outranks the setting it was written to.
    ///
    /// The one case that must not be reported as success: the download worked,
    /// the configuration was written, and the engine will still not be the one
    /// that runs. Naming both is the only way the reader can act on it.
    pub fn xray_release_shadowed(&self, tag: &str, path: &str, env: &str) -> String {
        match self.lang {
            Lang::En => format!(
                "Xray {tag} was saved to {path}, but {env} is set and outranks it, so that executable is still the one used; unset {env} to use the downloaded build"
            ),
            Lang::Zh => format!(
                "Xray {tag} 已保存到 {path}，但 {env} 已设置且优先级更高，仍会使用它指向的可执行文件；取消 {env} 后才会使用下载的构建"
            ),
        }
    }

    /// The engine arrived and the configuration would not take the path.
    ///
    /// The path is named because the binary is on disk: the reader can still put
    /// it in the field by hand, which is the next move this sentence leads to.
    pub fn xray_release_unrecorded(&self, tag: &str, path: &str, error: &str) -> String {
        match self.lang {
            Lang::En => format!(
                "Xray {tag} was downloaded to {path}, but the setting could not be saved: {error}"
            ),
            Lang::Zh => format!("Xray {tag} 已下载到 {path}，但无法保存设置：{error}"),
        }
    }

    /// The provenance line: which upstream build is on disk, and the hash to
    /// check it against.
    ///
    /// Written to the activity log rather than shown: it is the answer to a
    /// question asked later, and a banner carrying sixty-four hex characters is
    /// one nobody reads at the moment it appears.
    pub fn xray_release_recorded(
        &self,
        tag: &str,
        path: &str,
        sha256: &str,
        digest_url: &str,
    ) -> String {
        match self.lang {
            Lang::En => format!(
                "Xray {tag} installed at {path}; archive sha256 {sha256}, to check against {digest_url}"
            ),
            Lang::Zh => format!(
                "Xray {tag} 已安装到 {path}；压缩包 sha256 为 {sha256}，可与 {digest_url} 比对"
            ),
        }
    }

    /// What the download left out of the archive, in the sizes the caller has
    /// already written.
    ///
    /// Said rather than left implicit: the archive is sixty megabytes and what
    /// lands is one binary, and a reader who compares the two numbers should find
    /// the difference explained instead of wondering what failed.
    pub fn xray_release_trimmed(&self, kept: &str, skipped: &str) -> String {
        match self.lang {
            Lang::En => format!(
                "the engine and its licence were kept ({kept}); {skipped} of routing data was left in the archive"
            ),
            Lang::Zh => format!(
                "只保留可执行文件和许可证（{kept}）；{skipped} 的路由数据留在压缩包内未解压"
            ),
        }
    }

    /// Where the offered engines come from, naming the repository rather than
    /// spelling it out at the call site.
    pub fn xray_download_source(&self, repository: &str) -> String {
        match self.lang {
            Lang::En => format!("Builds published on github.com/{repository}."),
            Lang::Zh => format!("构建发布于 github.com/{repository}。"),
        }
    }

    /// What is at the configured engine path, for the line under the card.
    ///
    /// Four answers rather than two, because "there is a file" and "this window
    /// put it there" are different facts: the second is what a reader needs to
    /// find the build again, and only a path this program wrote down can say
    /// which release it was - including when that release has since been deleted
    /// from under it.
    pub fn xray_engine_state(&self, tag: Option<&str>, present: bool) -> String {
        match (self.lang, tag, present) {
            (Lang::En, Some(tag), true) => {
                format!("Downloaded from github.com/{REPOSITORY}: Xray {tag}.")
            }
            (Lang::Zh, Some(tag), true) => {
                format!("下载自 github.com/{REPOSITORY}：Xray {tag}。")
            }
            // The build is known and the file is not: said outright, because a
            // record of a download is the one thing that would otherwise leave a
            // reader believing there is an engine at the path.
            (Lang::En, Some(tag), false) => format!(
                "Downloaded from github.com/{REPOSITORY}: Xray {tag}, and it is not at the path \
                 above any more."
            ),
            (Lang::Zh, Some(tag), false) => format!(
                "下载自 github.com/{REPOSITORY}：Xray {tag}，但上面的路径上已经没有这个文件了。"
            ),
            (Lang::En, None, true) => "An engine is at the path above.".to_string(),
            (Lang::Zh, None, true) => "上面的路径上有一个引擎。".to_string(),
            (Lang::En, None, false) => {
                "Nothing is at the path above, so a profile with a proxy cannot start.".to_string()
            }
            (Lang::Zh, None, false) => {
                "上面的路径上什么都没有，使用代理的档案无法启动。".to_string()
            }
        }
    }
}

/// Where the engines this window downloaded come from.
///
/// Spelled here rather than passed in at every call, for the same reason
/// [`super::super::core_releases::REPOSITORY`] is one constant: a mirror is a
/// change in one place.
const REPOSITORY: &str = crate::xray_releases::REPOSITORY;
