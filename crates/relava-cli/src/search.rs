use crate::api_client::ApiClient;
use crate::install;

/// A single entry in the search results.
#[derive(Debug, serde::Serialize)]
pub struct SearchEntry {
    pub name: String,
    pub resource_type: String,
    pub version: String,
    pub description: String,
}

/// Result of the search command.
#[derive(Debug, serde::Serialize)]
pub struct SearchResult {
    pub query: String,
    pub results: Vec<SearchEntry>,
}

/// Options for the search command.
pub struct SearchOpts<'a> {
    pub server_url: &'a str,
    pub query: &'a str,
    pub resource_type: Option<&'a str>,
    pub json: bool,
}

/// Run `relava search <query> [--type <type>]`.
pub fn run(opts: &SearchOpts) -> Result<SearchResult, String> {
    // Validate the type filter if provided.
    if let Some(t) = opts.resource_type {
        install::parse_resource_type(t).map_err(|e| e.to_string())?;
    }

    let client = ApiClient::new(opts.server_url);
    let resources = client
        .search_resources(opts.query, opts.resource_type)
        .map_err(|e| e.to_string())?;

    let results: Vec<SearchEntry> = resources
        .into_iter()
        .map(|r| SearchEntry {
            name: r.name,
            resource_type: r.resource_type,
            version: r.latest_version.unwrap_or_default(),
            description: r.description.unwrap_or_default(),
        })
        .collect();

    if !opts.json {
        if results.is_empty() {
            println!("No results for '{}'.", opts.query);
        } else {
            print_table(&results);
        }
    }

    Ok(SearchResult {
        query: opts.query.to_string(),
        results,
    })
}

/// Print search results as a formatted table.
fn print_table(entries: &[SearchEntry]) {
    let rows: Vec<Vec<String>> = entries
        .iter()
        .map(|e| {
            let version = if e.version.is_empty() {
                "-"
            } else {
                &e.version
            };
            let snippet = truncate(&e.description, 60);
            vec![
                e.name.clone(),
                e.resource_type.clone(),
                version.to_string(),
                snippet,
            ]
        })
        .collect();

    println!(
        "{}",
        crate::output::table(&["Name", "Type", "Version", "Description"], &rows)
    );
}

/// Truncate a string to `max_len` characters, appending "..." if truncated.
fn truncate(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len.saturating_sub(3)).collect();
        format!("{truncated}...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string() {
        let long = "a".repeat(100);
        let result = truncate(&long, 60);
        assert!(result.len() <= 60);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn truncate_exact_length() {
        let s = "a".repeat(60);
        assert_eq!(truncate(&s, 60), s);
    }

    #[test]
    fn search_result_serializes_to_json() {
        let result = SearchResult {
            query: "denden".to_string(),
            results: vec![SearchEntry {
                name: "denden".to_string(),
                resource_type: "skill".to_string(),
                version: "1.0.0".to_string(),
                description: "Communication skill".to_string(),
            }],
        };
        let json = serde_json::to_string_pretty(&result).unwrap();
        assert!(json.contains("denden"));
        assert!(json.contains("1.0.0"));
    }

    #[test]
    fn search_fails_when_server_unreachable() {
        let opts = SearchOpts {
            server_url: "http://127.0.0.1:19999",
            query: "test",
            resource_type: None,
            json: true,
        };
        let err = run(&opts).unwrap_err();
        assert!(err.contains("Registry server not running"), "got: {err}");
    }

    #[test]
    fn search_with_invalid_type_fails() {
        let opts = SearchOpts {
            server_url: "http://127.0.0.1:19999",
            query: "test",
            resource_type: Some("invalid"),
            json: true,
        };
        let err = run(&opts).unwrap_err();
        assert!(
            err.contains("invalid") || err.contains("must be"),
            "got: {err}"
        );
    }

    // --- Mockito integration tests ---

    #[test]
    fn search_returns_results() {
        let mut server = mockito::Server::new();
        let _mock = server
            .mock("GET", "/api/v1/resources?q=denden")
            .with_status(200)
            .with_body(
                r#"[{"name":"denden","type":"skill","description":"Communication skill","latest_version":"1.0.0"}]"#,
            )
            .create();

        let opts = SearchOpts {
            server_url: &server.url(),
            query: "denden",
            resource_type: None,
            json: true,
        };
        let result = run(&opts).unwrap();
        assert_eq!(result.results.len(), 1);
        assert_eq!(result.results[0].name, "denden");
        assert_eq!(result.results[0].version, "1.0.0");
    }

    #[test]
    fn search_with_type_filter() {
        let mut server = mockito::Server::new();
        let _mock = server
            .mock("GET", "/api/v1/resources?q=debug&type=agent")
            .with_status(200)
            .with_body(r#"[{"name":"debugger","type":"agent","description":"Debug agent"}]"#)
            .create();

        let opts = SearchOpts {
            server_url: &server.url(),
            query: "debug",
            resource_type: Some("agent"),
            json: true,
        };
        let result = run(&opts).unwrap();
        assert_eq!(result.results.len(), 1);
        assert_eq!(result.results[0].resource_type, "agent");
    }

    #[test]
    fn search_empty_results() {
        let mut server = mockito::Server::new();
        let _mock = server
            .mock("GET", "/api/v1/resources?q=nonexistent")
            .with_status(200)
            .with_body(r#"[]"#)
            .create();

        let opts = SearchOpts {
            server_url: &server.url(),
            query: "nonexistent",
            resource_type: None,
            json: true,
        };
        let result = run(&opts).unwrap();
        assert!(result.results.is_empty());
    }
}
