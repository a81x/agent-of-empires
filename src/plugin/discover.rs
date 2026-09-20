//! GitHub plugin discovery over the `aoe-plugin` topic.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{bail, Result};
use aoe_plugin_api::{lucide_icon_name_ok, screenshot_path_ok, MAX_SCREENSHOTS};
use futures_util::{stream, StreamExt};
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::github::{GitHubClient, GitHubClientConfig, GitHubRepo, DEFAULT_USER_AGENT};

const RAW_PATH: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'|')
    .add(b'^')
    .add(b'\\')
    .add(b'[')
    .add(b']');

use super::featured::FeaturedIndex;
use super::source::PluginSource;

const PLUGIN_TOPIC: &str = "aoe-plugin";

const PROBE_TIMEOUT: Duration = Duration::from_secs(4);

const PROBE_PHASE_TIMEOUT: Duration = Duration::from_secs(6);

const PROBE_CONCURRENCY: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiscoveryBadge {
    Installed,
    Featured,
    Unvetted,
}

impl DiscoveryBadge {
    pub fn as_str(self) -> &'static str {
        match self {
            DiscoveryBadge::Installed => "installed",
            DiscoveryBadge::Featured => "featured",
            DiscoveryBadge::Unvetted => "unvetted",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscoveryResult {
    pub slug: String,
    pub html_url: String,
    pub description: Option<String>,
    pub stars: u64,
    pub badge: DiscoveryBadge,
    pub featured: bool,
    pub install_command: String,
    pub source_avatar_url: String,
}

pub async fn discover(query: Option<&str>) -> Result<Vec<DiscoveryResult>> {
    let client = client()?;

    let mut q = format!("topic:{PLUGIN_TOPIC} fork:false archived:false");
    if let Some(term) = query.map(str::trim).filter(|t| !t.is_empty()) {
        q.push(' ');
        q.push_str(term);
    }
    let repos = client.search_repositories(&q, 30).await?;

    let featured = FeaturedIndex::load()?;
    let installed = installed_slugs();
    let badged = badge_repos(repos, &featured, &installed);
    let probes = probe_unvetted(&badged).await;
    Ok(rank(drop_missing(badged, &probes)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManifestProbe {
    Present,
    Missing,
    Unknown,
}

fn classify(status: Option<StatusCode>) -> ManifestProbe {
    match status {
        Some(s) if s.is_success() => ManifestProbe::Present,
        Some(StatusCode::NOT_FOUND) => ManifestProbe::Missing,
        _ => ManifestProbe::Unknown,
    }
}

fn owner_repo(slug: &str) -> Option<(&str, &str)> {
    slug.strip_prefix("gh:")?.split_once('/')
}

async fn probe_manifest(http: &reqwest::Client, owner: &str, repo: &str) -> ManifestProbe {
    let url = raw_url(owner, repo, None, "aoe-plugin.toml");
    classify(http.head(url).send().await.ok().map(|r| r.status()))
}

async fn probe_unvetted(results: &[DiscoveryResult]) -> HashMap<String, ManifestProbe> {
    let targets: Vec<(String, String, String)> = results
        .iter()
        .filter(|r| r.badge == DiscoveryBadge::Unvetted)
        .filter_map(|r| {
            owner_repo(&r.slug)
                .map(|(owner, repo)| (r.slug.clone(), owner.to_string(), repo.to_string()))
        })
        .collect();
    let attempted = targets.len();
    if attempted == 0 {
        return HashMap::new();
    }
    let http = match reqwest::Client::builder()
        .user_agent(DEFAULT_USER_AGENT)
        .timeout(PROBE_TIMEOUT)
        .build()
    {
        Ok(http) => http,
        Err(_) => return HashMap::new(),
    };

    let mut probes = stream::iter(targets.into_iter().map(|(slug, owner, repo)| {
        let http = &http;
        async move { (slug, probe_manifest(http, &owner, &repo).await) }
    }))
    .buffer_unordered(PROBE_CONCURRENCY);

    let deadline = tokio::time::Instant::now() + PROBE_PHASE_TIMEOUT;
    let mut out = HashMap::new();
    while let Ok(Some((slug, probe))) = tokio::time::timeout_at(deadline, probes.next()).await {
        out.insert(slug, probe);
    }
    log_probes(attempted, &out);
    out
}

fn log_probes(attempted: usize, probes: &HashMap<String, ManifestProbe>) {
    let count = |want: ManifestProbe| probes.values().filter(|p| **p == want).count();
    let present = count(ManifestProbe::Present);
    let missing = count(ManifestProbe::Missing);
    let unknown = attempted - present - missing;
    if present == 0 && missing == 0 {
        tracing::warn!(
            target: "plugin.discover",
            unknown,
            "every manifest probe was inconclusive; raw.githubusercontent.com may be unreachable, so topic-collision results are not filtered"
        );
    } else {
        tracing::debug!(
            target: "plugin.discover",
            present,
            missing,
            unknown,
            "manifest probes"
        );
    }
}

fn drop_missing(
    results: Vec<DiscoveryResult>,
    probes: &HashMap<String, ManifestProbe>,
) -> Vec<DiscoveryResult> {
    results
        .into_iter()
        .filter(|r| probes.get(&r.slug) != Some(&ManifestProbe::Missing))
        .collect()
}

fn installed_slugs() -> Vec<String> {
    super::registry()
        .all()
        .iter()
        .filter_map(|p| p.source.as_deref())
        .filter_map(|s| PluginSource::parse(s).ok())
        .filter(|s| matches!(s, PluginSource::Github { .. }))
        .map(|s| s.slug().to_ascii_lowercase())
        .collect()
}

fn badge_repos(
    repos: Vec<GitHubRepo>,
    featured: &FeaturedIndex,
    installed: &[String],
) -> Vec<DiscoveryResult> {
    repos
        .into_iter()
        .filter_map(|repo| {
            if repo.full_name.split('/').filter(|s| !s.is_empty()).count() != 2 {
                return None;
            }
            let slug = format!("gh:{}", repo.full_name);
            let normalized = slug.to_ascii_lowercase();
            let is_installed = installed.contains(&normalized);
            let is_featured = featured.is_featured_source(&slug);
            let badge = if is_installed {
                DiscoveryBadge::Installed
            } else if is_featured {
                DiscoveryBadge::Featured
            } else {
                DiscoveryBadge::Unvetted
            };
            let owner = repo.full_name.split('/').next().unwrap_or_default();
            Some(DiscoveryResult {
                install_command: format!("aoe plugin install {slug}"),
                slug,
                html_url: repo.html_url,
                featured: is_featured,
                description: repo.description.filter(|d| !d.is_empty()),
                stars: repo.stargazers_count,
                badge,
                source_avatar_url: format!("https://github.com/{owner}.png?size=64"),
            })
        })
        .collect()
}

fn rank(mut results: Vec<DiscoveryResult>) -> Vec<DiscoveryResult> {
    results.sort_by(|a, b| {
        b.featured
            .cmp(&a.featured)
            .then(b.stars.cmp(&a.stars))
            .then(a.slug.cmp(&b.slug))
    });
    results
}

fn client() -> Result<GitHubClient> {
    Ok(GitHubClient::unauthenticated(GitHubClientConfig {
        api_base: api_base(),
        user_agent: DEFAULT_USER_AGENT.to_string(),
        timeout: Duration::from_secs(30),
    })?)
}

fn api_base() -> String {
    std::env::var("AOE_UPDATE_API_BASE")
        .unwrap_or_else(|_| crate::github::DEFAULT_GITHUB_API_BASE.to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct DetailManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub api_version: u32,
    pub capabilities: Vec<String>,
    pub ui_contributions: Vec<UiSlotView>,
    pub screenshots: Vec<ScreenshotView>,
    pub icon: Option<String>,
    pub icon_asset_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UiSlotView {
    pub slot: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScreenshotView {
    pub src: String,
    pub alt: String,
    pub caption: String,
}

fn resolve_screenshots(
    api_version: u32,
    raws: Vec<RawScreenshot>,
    owner: &str,
    repo: &str,
    reference: Option<&str>,
) -> Vec<ScreenshotView> {
    if api_version < 5 {
        return Vec::new();
    }
    raws.into_iter()
        .filter(|s| screenshot_path_ok(&s.path) && !s.alt.trim().is_empty())
        .take(MAX_SCREENSHOTS)
        .map(|s| ScreenshotView {
            src: raw_url(owner, repo, reference, &s.path),
            alt: s.alt,
            caption: s.caption,
        })
        .collect()
}

fn resolve_icon_name(api_version: u32, icon: Option<String>) -> Option<String> {
    if api_version < 7 {
        return None;
    }
    icon.filter(|i| lucide_icon_name_ok(i))
}

fn resolve_icon_asset(
    api_version: u32,
    path: Option<String>,
    owner: &str,
    repo: &str,
    reference: Option<&str>,
) -> Option<String> {
    if api_version < 7 {
        return None;
    }
    let path = path?;
    screenshot_path_ok(&path).then(|| raw_url(owner, repo, reference, &path))
}

fn raw_url(owner: &str, repo: &str, reference: Option<&str>, path: &str) -> String {
    let reference = reference.unwrap_or("HEAD");
    format!(
        "https://raw.githubusercontent.com/{}/{}/{}/{}",
        utf8_percent_encode(owner, RAW_PATH),
        utf8_percent_encode(repo, RAW_PATH),
        utf8_percent_encode(reference, RAW_PATH),
        utf8_percent_encode(path, RAW_PATH),
    )
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginDetail {
    pub source: String,
    pub manifest: Option<DetailManifest>,
    pub manifest_error: Option<String>,
    pub release_tags: Vec<String>,
}

#[derive(Deserialize)]
struct RawManifest {
    id: String,
    name: String,
    version: String,
    #[serde(default)]
    description: String,
    api_version: u32,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    ui: Vec<RawUi>,
    #[serde(default)]
    screenshots: Vec<RawScreenshot>,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    icon_asset: Option<String>,
}

#[derive(Deserialize)]
struct RawUi {
    slot: String,
    id: String,
}

#[derive(Deserialize)]
struct RawScreenshot {
    #[serde(default)]
    path: String,
    #[serde(default)]
    alt: String,
    #[serde(default)]
    caption: String,
}

pub async fn details(source: &str) -> Result<PluginDetail> {
    let parsed = PluginSource::parse(source)?;
    let PluginSource::Github { owner, repo, .. } = &parsed else {
        bail!("details are only available for a gh:owner/repo source");
    };
    let reference = parsed.reference();
    let client = client()?;

    let manifest = match client
        .get_repo_file(owner, repo, "aoe-plugin.toml", reference)
        .await
    {
        Ok(text) => toml::from_str::<RawManifest>(&text)
            .map(|m| DetailManifest {
                id: m.id,
                name: m.name,
                version: m.version,
                description: m.description,
                api_version: m.api_version,
                capabilities: m.capabilities,
                ui_contributions: m
                    .ui
                    .into_iter()
                    .map(|u| UiSlotView {
                        slot: u.slot,
                        id: u.id,
                    })
                    .collect(),
                screenshots: resolve_screenshots(
                    m.api_version,
                    m.screenshots,
                    owner,
                    repo,
                    reference,
                ),
                icon: resolve_icon_name(m.api_version, m.icon),
                icon_asset_url: resolve_icon_asset(
                    m.api_version,
                    m.icon_asset,
                    owner,
                    repo,
                    reference,
                ),
            })
            .map_err(|e| format!("aoe-plugin.toml is invalid: {e}")),
        Err(e) => Err(format!("{e}")),
    };

    let release_tags = client
        .list_releases(owner, repo, 30)
        .await
        .map(|rs| rs.into_iter().map(|r| r.tag_name).collect())
        .unwrap_or_default();

    let (manifest, manifest_error) = match manifest {
        Ok(m) => (Some(m), None),
        Err(e) => (None, Some(e)),
    };
    Ok(PluginDetail {
        source: parsed.slug(),
        manifest,
        manifest_error,
        release_tags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(full_name: &str, stars: u64) -> GitHubRepo {
        GitHubRepo {
            full_name: full_name.to_string(),
            html_url: format!("https://github.com/{full_name}"),
            description: Some("a plugin".to_string()),
            stargazers_count: stars,
            topics: vec!["aoe-plugin".to_string()],
        }
    }

    fn featured(slug: &str) -> FeaturedIndex {
        FeaturedIndex::from_toml_str(&format!(
            "[plugins.\"x.y\"]\nsource = \"{slug}\"\nversions = {{ \"1.0\" = \"sha256:abc\" }}\n"
        ))
        .unwrap()
    }

    #[test]
    fn badges_installed_featured_unvetted() {
        let repos = vec![
            repo("acme/installed", 5),
            repo("acme/vetted", 10),
            repo("acme/random", 100),
        ];
        let index = featured("gh:acme/vetted");
        let installed = vec!["gh:acme/installed".to_string()];
        let out = badge_repos(repos, &index, &installed);
        let by_slug = |slug: &str| out.iter().find(|r| r.slug == slug).unwrap().badge;
        assert_eq!(by_slug("gh:acme/installed"), DiscoveryBadge::Installed);
        assert_eq!(by_slug("gh:acme/vetted"), DiscoveryBadge::Featured);
        assert_eq!(by_slug("gh:acme/random"), DiscoveryBadge::Unvetted);
    }

    #[test]
    fn installed_match_is_case_insensitive() {
        let repos = vec![repo("Acme/Widget", 1)];
        let installed = vec!["gh:acme/widget".to_string()];
        let out = badge_repos(repos, &FeaturedIndex::default(), &installed);
        assert_eq!(out[0].badge, DiscoveryBadge::Installed);
    }

    #[test]
    fn ranks_featured_first_then_stars() {
        let repos = vec![repo("acme/popular", 999), repo("acme/vetted", 1)];
        let index = featured("gh:acme/vetted");
        let out = rank(badge_repos(repos, &index, &[]));
        assert_eq!(out[0].slug, "gh:acme/vetted");
        assert_eq!(out[1].slug, "gh:acme/popular");
    }

    #[test]
    fn installed_and_featured_still_ranks_featured() {
        let repos = vec![repo("acme/popular", 999), repo("acme/vetted", 1)];
        let index = featured("gh:acme/vetted");
        let installed = vec!["gh:acme/vetted".to_string()];
        let out = rank(badge_repos(repos, &index, &installed));
        assert_eq!(out[0].slug, "gh:acme/vetted");
        assert_eq!(out[0].badge, DiscoveryBadge::Installed);
        assert!(out[0].featured);
    }

    #[test]
    fn drops_non_owner_repo_results() {
        let repos = vec![repo("not-a-slug", 1), repo("a/b/c", 1)];
        let out = badge_repos(repos, &FeaturedIndex::default(), &[]);
        assert!(out.is_empty());
    }

    #[test]
    fn classify_maps_only_404_to_missing() {
        let cases = [
            (Some(StatusCode::OK), ManifestProbe::Present),
            (Some(StatusCode::NOT_FOUND), ManifestProbe::Missing),
            (Some(StatusCode::FORBIDDEN), ManifestProbe::Unknown),
            (Some(StatusCode::TOO_MANY_REQUESTS), ManifestProbe::Unknown),
            (
                Some(StatusCode::INTERNAL_SERVER_ERROR),
                ManifestProbe::Unknown,
            ),
            (None, ManifestProbe::Unknown),
        ];
        for (status, expected) in cases {
            assert_eq!(classify(status), expected, "{status:?}");
        }
    }

    #[test]
    fn drops_only_the_results_a_probe_proved_missing() {
        let repos = vec![
            repo("acme/missing", 1),
            repo("acme/present", 1),
            repo("acme/unknown", 1),
            repo("acme/unprobed", 1),
            repo("acme/installed", 1),
            repo("acme/vetted", 1),
        ];
        let index = featured("gh:acme/vetted");
        let installed = vec!["gh:acme/installed".to_string()];
        let badged = badge_repos(repos, &index, &installed);
        let probes = HashMap::from([
            ("gh:acme/missing".to_string(), ManifestProbe::Missing),
            ("gh:acme/present".to_string(), ManifestProbe::Present),
            ("gh:acme/unknown".to_string(), ManifestProbe::Unknown),
        ]);
        let kept: Vec<String> = drop_missing(badged, &probes)
            .into_iter()
            .map(|r| r.slug)
            .collect();
        assert!(
            !kept.contains(&"gh:acme/missing".to_string()),
            "a confirmed-missing manifest must drop the result: {kept:?}"
        );
        assert_eq!(kept.len(), 5, "everything else fails open: {kept:?}");
    }

    #[test]
    fn a_page_of_missing_manifests_yields_the_empty_state() {
        let badged = badge_repos(
            vec![repo("acme/a", 1), repo("acme/b", 1)],
            &FeaturedIndex::default(),
            &[],
        );
        let probes = HashMap::from([
            ("gh:acme/a".to_string(), ManifestProbe::Missing),
            ("gh:acme/b".to_string(), ManifestProbe::Missing),
        ]);
        assert!(drop_missing(badged, &probes).is_empty());
    }

    #[test]
    fn the_manifest_probe_targets_the_raw_cdn_not_the_api() {
        let url = raw_url("acme", "widget", None, "aoe-plugin.toml");
        assert_eq!(
            url,
            "https://raw.githubusercontent.com/acme/widget/HEAD/aoe-plugin.toml"
        );
        assert!(!url.starts_with(&api_base()), "{url}");
    }

    #[test]
    fn detail_manifest_parse_tolerates_newer_api_version_and_unknown_keys() {
        let toml = r#"
id = "acme.future"
name = "Future"
version = "9.9.9"
api_version = 99
description = "from the future"
capabilities = ["net"]
some_unknown_future_key = true

[[ui]]
slot = "status-bar"
id = "s"
"#;
        let m: RawManifest = toml::from_str(toml).expect("lenient parse");
        assert_eq!(m.version, "9.9.9");
        assert_eq!(m.api_version, 99);
        assert_eq!(m.capabilities, vec!["net"]);
        assert_eq!(m.ui.len(), 1);
        assert_eq!(m.ui[0].slot, "status-bar");
    }

    #[test]
    fn raw_url_defaults_to_head_and_encodes_path() {
        assert_eq!(
            raw_url("acme", "widget", None, "docs/shots/a.png"),
            "https://raw.githubusercontent.com/acme/widget/HEAD/docs/shots/a.png"
        );
        assert_eq!(
            raw_url("acme", "widget", Some("v1.2.0"), "media/cool demo.gif"),
            "https://raw.githubusercontent.com/acme/widget/v1.2.0/media/cool%20demo.gif"
        );
    }

    #[test]
    fn detail_manifest_parses_screenshots_and_drops_bad_entries() {
        let toml = r#"
id = "acme.widget"
name = "Widget"
version = "1.0.0"
api_version = 5

[[screenshots]]
path = "docs/a.png"
alt = "good"

[[screenshots]]
path = "https://tracker.example.com/x.png"
alt = "bad url"

[[screenshots]]
path = "docs/b.png"
alt = "   "
"#;
        let m: RawManifest = toml::from_str(toml).expect("lenient parse");
        let kept = resolve_screenshots(m.api_version, m.screenshots, "acme", "widget", None);
        assert_eq!(kept.len(), 1);
        assert_eq!(
            kept[0].src,
            "https://raw.githubusercontent.com/acme/widget/HEAD/docs/a.png"
        );
    }

    #[test]
    fn screenshots_gated_out_below_api_version_5() {
        let toml = r#"
id = "acme.widget"
name = "Widget"
version = "1.0.0"
api_version = 4

[[screenshots]]
path = "docs/a.png"
alt = "good"
"#;
        let m: RawManifest = toml::from_str(toml).expect("lenient parse");
        let kept = resolve_screenshots(m.api_version, m.screenshots, "acme", "widget", None);
        assert!(kept.is_empty(), "v4 must not expose screenshots");
    }

    #[test]
    fn install_command_uses_the_slug() {
        let out = badge_repos(vec![repo("acme/widget", 1)], &FeaturedIndex::default(), &[]);
        assert_eq!(out[0].install_command, "aoe plugin install gh:acme/widget");
    }

    #[test]
    fn source_avatar_url_derives_from_the_owner_with_no_extra_request() {
        let out = badge_repos(vec![repo("acme/widget", 1)], &FeaturedIndex::default(), &[]);
        assert_eq!(
            out[0].source_avatar_url,
            "https://github.com/acme.png?size=64"
        );
    }

    #[test]
    fn detail_manifest_parses_icon_and_resolves_icon_asset() {
        let toml = r#"
id = "acme.widget"
name = "Widget"
version = "1.0.0"
api_version = 7
icon = "git-branch"
icon_asset = "assets/icon.png"
"#;
        let m: RawManifest = toml::from_str(toml).expect("lenient parse");
        assert_eq!(
            resolve_icon_name(m.api_version, m.icon.clone()).as_deref(),
            Some("git-branch")
        );
        let url = resolve_icon_asset(m.api_version, m.icon_asset, "acme", "widget", None);
        assert_eq!(
            url.as_deref(),
            Some("https://raw.githubusercontent.com/acme/widget/HEAD/assets/icon.png")
        );
    }

    #[test]
    fn icon_name_gated_out_below_api_version_7() {
        let toml = r#"
id = "acme.widget"
name = "Widget"
version = "1.0.0"
api_version = 6
icon = "git-branch"
"#;
        let m: RawManifest = toml::from_str(toml).expect("lenient parse");
        assert!(
            resolve_icon_name(m.api_version, m.icon).is_none(),
            "v6 must not expose icon"
        );
    }

    #[test]
    fn icon_name_drops_an_invalid_name() {
        let toml = r#"
id = "acme.widget"
name = "Widget"
version = "1.0.0"
api_version = 7
icon = "GitHub"
"#;
        let m: RawManifest = toml::from_str(toml).expect("lenient parse");
        assert!(
            resolve_icon_name(m.api_version, m.icon).is_none(),
            "a non-kebab-case name must be dropped, not surfaced to the client"
        );
    }

    #[test]
    fn icon_asset_gated_out_below_api_version_7() {
        let toml = r#"
id = "acme.widget"
name = "Widget"
version = "1.0.0"
api_version = 6
icon_asset = "assets/icon.png"
"#;
        let m: RawManifest = toml::from_str(toml).expect("lenient parse");
        let url = resolve_icon_asset(m.api_version, m.icon_asset, "acme", "widget", None);
        assert!(url.is_none(), "v6 must not expose icon_asset");
    }

    #[test]
    fn icon_asset_drops_an_invalid_path() {
        let toml = r#"
id = "acme.widget"
name = "Widget"
version = "1.0.0"
api_version = 7
icon_asset = "https://tracker.example.com/x.png"
"#;
        let m: RawManifest = toml::from_str(toml).expect("lenient parse");
        let url = resolve_icon_asset(m.api_version, m.icon_asset, "acme", "widget", None);
        assert!(url.is_none(), "an absolute URL path must be dropped");
    }
}
