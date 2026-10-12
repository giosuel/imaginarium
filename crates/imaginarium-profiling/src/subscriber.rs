// SPDX-License-Identifier: AGPL-3.0-or-later

//! Process-wide `tracing` subscriber setup.

use std::io::IsTerminal;

use thiserror::Error;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Environment variable holding the log filter, e.g.
/// `IMAGINARIUM_LOG=info,imaginarium_render=trace`. If it isn't set,
/// `RUST_LOG` is used, then [`DEFAULT_FILTER`].
pub const FILTER_ENV: &str = "IMAGINARIUM_LOG";

/// Filter used when neither the config nor the environment sets one.
pub const DEFAULT_FILTER: &str = "info";

/// Output format for log lines (written to stderr).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogFormat {
    /// One line per event, compact. Good default for terminals.
    #[default]
    Compact,
    /// Multi-line, human-friendly, with span context. Good for debugging.
    Pretty,
    /// One JSON object per line. Good for CI, bug reports and tooling.
    Json,
}

/// How to set up tracing for the process. Build it from settings or CLI
/// flags, then pass it to [`init`].
#[derive(Clone, Debug, Default)]
pub struct TracingConfig {
    /// Filter directives in `EnvFilter` syntax (e.g. `"warn,imaginarium=debug"`).
    /// `None` reads [`FILTER_ENV`], then `RUST_LOG`, then [`DEFAULT_FILTER`].
    pub filter: Option<String>,
    /// Log line format.
    pub format: LogFormat,
    /// Also stream spans to the Tracy profiler. Requires the `tracy` feature.
    pub tracy: bool,
}

/// Why [`init`] failed.
#[derive(Debug, Error)]
pub enum InitError {
    /// The filter directives couldn't be parsed.
    #[error("invalid tracing filter {directives:?}: {source}")]
    InvalidFilter {
        /// The directives that failed to parse.
        directives: String,
        /// Parser error.
        #[source]
        source: tracing_subscriber::filter::ParseError,
    },
    /// Tracy was requested, but this build doesn't include the `tracy` feature.
    #[error("Tracy profiling requested, but this build lacks the `tracy` feature")]
    TracyUnavailable,
    /// A global subscriber is already installed (`init` called twice?).
    #[error("a global tracing subscriber is already set: {0}")]
    AlreadyInitialized(#[from] tracing_subscriber::util::TryInitError),
}

/// Installs the global tracing subscriber. Call once, early in `main`.
///
/// Logs go to stderr, with ANSI colors only when stderr is a terminal. The
/// filter applies to every output, including Tracy.
///
/// ```
/// use imaginarium_profiling::subscriber::{init, LogFormat, TracingConfig};
///
/// init(&TracingConfig { filter: Some("warn".into()), format: LogFormat::Compact, tracy: false })?;
/// tracing::warn!("visible");
/// tracing::info!("filtered out");
/// # Ok::<(), imaginarium_profiling::subscriber::InitError>(())
/// ```
pub fn init(config: &TracingConfig) -> Result<(), InitError> {
    let directives = resolve_filter(config.filter.as_deref(), |name| std::env::var(name).ok());
    let filter = EnvFilter::try_new(&directives).map_err(|source| InitError::InvalidFilter {
        directives: directives.clone(),
        source,
    })?;

    // Each layer is built separately: `fmt::layer()` is generic over the
    // subscriber it sits on, and each layer below sits on a different stack.
    let ansi = std::io::stderr().is_terminal();
    let compact = (config.format == LogFormat::Compact).then(|| {
        tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_ansi(ansi)
            .compact()
    });
    let pretty = (config.format == LogFormat::Pretty).then(|| {
        tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_ansi(ansi)
            .pretty()
    });
    let json = (config.format == LogFormat::Json).then(|| {
        tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .json()
    });

    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(compact)
        .with(pretty)
        .with(json);

    #[cfg(feature = "tracy")]
    {
        // With the `manual-lifetime` client feature, creating the layer is
        // what starts the Tracy client. So it only runs when requested.
        let tracy = config
            .tracy
            .then(|| tracing_tracy::TracyLayer::new(tracing_tracy::DefaultConfig::default()));
        registry.with(tracy).try_init()?;
    }
    #[cfg(not(feature = "tracy"))]
    {
        if config.tracy {
            return Err(InitError::TracyUnavailable);
        }
        registry.try_init()?;
    }
    Ok(())
}

/// Picks the filter directives: explicit config, then [`FILTER_ENV`], then
/// `RUST_LOG`, then [`DEFAULT_FILTER`]. Empty values count as unset.
fn resolve_filter(explicit: Option<&str>, env: impl Fn(&str) -> Option<String>) -> String {
    let set = |directives: &String| !directives.trim().is_empty();
    explicit
        .map(str::to_owned)
        .filter(set)
        .or_else(|| env(FILTER_ENV).filter(set))
        .or_else(|| env("RUST_LOG").filter(set))
        .unwrap_or_else(|| DEFAULT_FILTER.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_with(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |name| vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
    }

    #[test]
    fn explicit_filter_wins() {
        let env = env_with(&[(FILTER_ENV, "debug"), ("RUST_LOG", "trace")]);
        assert_eq!(resolve_filter(Some("warn"), env), "warn");
    }

    #[test]
    fn own_env_var_beats_rust_log() {
        let env = env_with(&[(FILTER_ENV, "debug"), ("RUST_LOG", "trace")]);
        assert_eq!(resolve_filter(None, env), "debug");
        assert_eq!(
            resolve_filter(None, env_with(&[("RUST_LOG", "trace")])),
            "trace"
        );
    }

    #[test]
    fn empty_values_fall_through_to_the_next_source() {
        let env = env_with(&[(FILTER_ENV, ""), ("RUST_LOG", "debug")]);
        assert_eq!(resolve_filter(None, env), "debug");
        let env = env_with(&[(FILTER_ENV, "trace")]);
        assert_eq!(resolve_filter(Some(" "), env), "trace");
    }

    #[test]
    fn falls_back_to_default() {
        assert_eq!(resolve_filter(None, env_with(&[])), DEFAULT_FILTER);
        assert_eq!(resolve_filter(Some("  "), env_with(&[])), DEFAULT_FILTER);
    }

    #[test]
    fn invalid_filter_is_reported() {
        let config = TracingConfig {
            filter: Some("info,[".into()),
            ..TracingConfig::default()
        };
        assert!(matches!(
            init(&config),
            Err(InitError::InvalidFilter { .. })
        ));
    }

    /// Starts a real Tracy client (listening on localhost only). This is the
    /// only test that installs the global subscriber.
    #[cfg(feature = "tracy")]
    #[test]
    fn tracy_layer_starts() {
        let config = TracingConfig {
            filter: Some("warn".into()),
            tracy: true,
            ..TracingConfig::default()
        };
        init(&config).unwrap();
        let _span = tracing::warn_span!("tracy_smoke_test").entered();
        tracing::warn!("event inside a span");
    }

    #[cfg(not(feature = "tracy"))]
    #[test]
    fn tracy_without_feature_is_an_error() {
        let config = TracingConfig {
            tracy: true,
            ..TracingConfig::default()
        };
        assert!(matches!(init(&config), Err(InitError::TracyUnavailable)));
    }
}
