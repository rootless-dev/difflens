//! Where a project lives on a server, read from its git remote URL. Only the
//! host name and the repository path matter: user names, passwords and ports
//! in the URL are dropped, so a token written into a URL never travels on.

/// A repository on a server, such as `bitbucket.org` / `acme` /
/// `payments-api`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRepo {
    /// Lowercase host name, without user or port.
    pub host: String,
    /// Workspace, user or group. Nested groups keep their slashes.
    pub owner: String,
    /// Repository slug, without `.git`.
    pub name: String,
}

/// Parses the URL forms git accepts for a server: `scheme://[user@]host[:port]/path`
/// (https, http, ssh, git, git+ssh) and the scp-like `[user@]host:path`.
/// `None` for local paths, `file://` URLs and anything without an owner and
/// a name.
pub fn parse(url: &str) -> Option<RemoteRepo> {
    let url = url.trim();
    let (authority, path) = match url.split_once("://") {
        Some((scheme, rest)) => {
            let scheme = scheme.to_ascii_lowercase();
            if !matches!(
                scheme.as_str(),
                "https" | "http" | "ssh" | "git" | "git+ssh" | "ssh+git"
            ) {
                return None;
            }
            let (authority, path) = rest.split_once('/')?;
            (authority, path)
        }
        None => scp_like(url)?,
    };
    let host = host_of(authority)?;
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, name) = path.rsplit_once('/')?;
    let owner = owner.trim_start_matches('~');
    let segment = |text: &str| {
        !text.is_empty() && !text.contains(|c: char| c.is_whitespace() || c == ':' || c == '@')
    };
    if !owner.split('/').all(segment) || !segment(name) {
        return None;
    }
    Some(RemoteRepo {
        host,
        owner: owner.to_string(),
        name: name.to_string(),
    })
}

/// `[user@]host:path`, which git only reads as such when no slash comes
/// before the first colon. A single letter before it is a Windows drive.
fn scp_like(url: &str) -> Option<(&str, &str)> {
    let (authority, path) = url.split_once(':')?;
    if authority.contains('/') || path.starts_with("//") {
        return None;
    }
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if host.len() < 2 {
        return None;
    }
    Some((authority, path))
}

/// The host of `[user[:password]@]host[:port]`, lowercased.
fn host_of(authority: &str) -> Option<String> {
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = match host.rsplit_once(':') {
        Some((name, port)) if port.bytes().all(|byte| byte.is_ascii_digit()) => name,
        _ => host,
    };
    let valid = !host.is_empty()
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-');
    valid.then(|| host.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(host: &str, owner: &str, name: &str) -> Option<RemoteRepo> {
        Some(RemoteRepo {
            host: host.to_string(),
            owner: owner.to_string(),
            name: name.to_string(),
        })
    }

    #[test]
    fn reads_ssh_and_https_remotes() {
        let expected = repo("bitbucket.org", "acme", "payments-api");
        for url in [
            "git@bitbucket.org:acme/payments-api.git",
            "git@bitbucket.org:acme/payments-api",
            "bitbucket.org:acme/payments-api.git",
            "ssh://git@bitbucket.org/acme/payments-api.git",
            "ssh://git@bitbucket.org:22/acme/payments-api.git",
            "https://bitbucket.org/acme/payments-api.git",
            "https://dana@bitbucket.org/acme/payments-api.git",
            "https://x-token-auth:s3cr3t@bitbucket.org/acme/payments-api.git",
            "HTTPS://BitBucket.org/acme/payments-api/",
            "  https://bitbucket.org/acme/payments-api  ",
        ] {
            assert_eq!(parse(url), expected, "{url}");
        }
    }

    #[test]
    fn reads_github_remotes_including_ssh_over_https_port() {
        let expected = |host: &str| repo(host, "acme", "payments-api");
        for url in [
            "git@github.com:acme/payments-api.git",
            "github.com:acme/payments-api",
            "ssh://git@github.com/acme/payments-api.git",
            "https://github.com/acme/payments-api.git",
            "https://x-access-token:ghp_s3cr3t@github.com/acme/payments-api.git",
            "git+ssh://git@github.com/acme/payments-api",
        ] {
            assert_eq!(parse(url), expected("github.com"), "{url}");
        }
        assert_eq!(
            parse("ssh://git@ssh.github.com:443/acme/payments-api.git"),
            expected("ssh.github.com")
        );
        assert_eq!(
            parse("https://www.github.com/acme/payments-api"),
            expected("www.github.com")
        );
    }

    #[test]
    fn keeps_nested_groups_and_other_hosts() {
        assert_eq!(
            parse("git@gitlab.com:acme/platform/payments-api.git"),
            repo("gitlab.com", "acme/platform", "payments-api")
        );
        assert_eq!(
            parse("https://github.com/acme/payments-api"),
            repo("github.com", "acme", "payments-api")
        );
        assert_eq!(
            parse("git://git.example.com:9418/team/tool.git"),
            repo("git.example.com", "team", "tool")
        );
    }

    #[test]
    fn local_and_incomplete_remotes_are_not_servers() {
        for url in [
            "/srv/git/payments-api.git",
            "../payments-api",
            "file:///srv/git/payments-api.git",
            "C:/repos/payments-api",
            "https://bitbucket.org/payments-api",
            "git@bitbucket.org:payments-api.git",
            "https://bitbucket.org/",
            "ext::sh -c touch% /tmp/x",
            "",
        ] {
            assert_eq!(parse(url), None, "{url}");
        }
    }
}
