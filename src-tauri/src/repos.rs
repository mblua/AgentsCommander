use std::path::Path;

/// Derive extended repo name as "parent/repo" from an absolute path.
/// Always uses forward slash as separator regardless of OS.
pub(crate) fn derive_repo_name(path: &Path) -> Option<String> {
    let file_name = path.file_name().and_then(|n| n.to_str())?;
    let name = match path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
    {
        Some(parent) => format!("{}/{}", parent, file_name),
        None => file_name.to_string(),
    };
    Some(name)
}

/// #943 - drop `user[:password]@` from a scheme-form remote URL.
///
/// `git remote get-url` returns whatever `.git/config` holds, and a repo cloned
/// with a token carries `https://<user>:<ghp_...>@github.com/o/r.git`. The caller
/// only needs host + owner + repo, so the secret is destroyed here and never
/// crosses the IPC boundary into the webview (nor into devtools, nor into any
/// future console log). scp-like remotes (`git@github.com:o/r.git`) are returned
/// untouched: that `git` is an ssh user name, not a credential, and the frontend
/// parser ignores it.
pub(crate) fn strip_userinfo(remote: &str) -> String {
    let Some(scheme_end) = remote.find("://") else {
        return remote.to_string();
    };
    let authority_start = scheme_end + 3;
    let authority_end = remote[authority_start..]
        .find('/')
        .map(|i| authority_start + i)
        .unwrap_or(remote.len());
    let authority = &remote[authority_start..authority_end];
    match authority.rfind('@') {
        Some(at) => format!(
            "{}{}{}",
            &remote[..authority_start],
            &authority[at + 1..],
            &remote[authority_end..]
        ),
        None => remote.to_string(),
    }
}

#[cfg(test)]
mod tests;
