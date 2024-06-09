use std::process::Command;

fn main() {
    let output = Command::new("git")
        .arg("remote")
        .arg("get-url")
        .arg("origin")
        .output()
        .expect("failed to run git");
    let mut found_git_url = false;
    if output.status.success() {
        if let Ok(mut url) = String::from_utf8(output.stdout) {
            let mut trimmed_url = url.trim();
            if let Some(suffix) = url.strip_prefix("git@github.com:") {
                url = format!("https://github.com/{suffix}");
                trimmed_url = &url;
            }
            println!("cargo::rustc-env=GIT_URL={trimmed_url}");
            found_git_url = true;
        }
    }
    if !found_git_url {
        println!("cargo::rustc-env=GIT_URL=unknown");
    }
}
