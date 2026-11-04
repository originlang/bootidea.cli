//! 包的签名与发布者信任库。

use crate::cli::TrustCommand;
use crate::config::Settings;
use crate::engine;
use anyhow::{Context, Result, bail};
use originlang_ext_signing::{KeyFile, sign_package};
use std::fs;
use std::path::Path;

/// 写一对签名密钥。
pub fn keygen(out: &Path, publisher: &str) -> Result<()> {
    if out.exists() {
        bail!("{} 已存在；拒绝覆盖签名密钥", out.display());
    }
    let file = KeyFile::generate(publisher).map_err(|error| anyhow::anyhow!("{error}"))?;
    if let Some(parent) = out.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).with_context(|| format!("无法创建 {}", parent.display()))?;
    }
    file.save(out).map_err(|error| anyhow::anyhow!("{error}"))?;

    println!("发布者:   {}", file.publisher);
    println!("公钥:     {}", file.public_key);
    println!("密钥文件: {}", out.display());
    println!();
    println!(
        "在本机信任该公钥:  bootidea trust add {} --name {}",
        file.public_key, file.publisher
    );
    println!("密钥文件是包的签名凭据，请妥善保管，不要放进包目录。");
    Ok(())
}

/// 签名一个包目录，写出 `extension.sig`。
pub fn sign(dir: &Path, key_path: &Path, publisher: Option<&str>) -> Result<()> {
    let key_file = KeyFile::load(key_path)
        .map_err(|error| anyhow::anyhow!("无法读取 {}: {error}", key_path.display()))?;
    let publisher = publisher.unwrap_or(&key_file.publisher);
    let key = key_file
        .secret()
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let document = sign_package(dir, &key, publisher)
        .map_err(|error| anyhow::anyhow!("无法签名 {}: {error}", dir.display()))?;
    println!(
        "已签名 {}，发布者 '{}'（{}）",
        dir.display(),
        document.publisher,
        &document.public_key[..16]
    );
    Ok(())
}

/// 读或改发布者信任库。
pub fn trust(settings: &Settings, command: TrustCommand) -> Result<()> {
    engine::with_host(settings, false, |host| match command {
        TrustCommand::List => {
            let entries = host.trusted_publishers();
            if entries.is_empty() {
                println!(
                    "{} 中没有受信任的发布者",
                    host.paths().publishers_path().display()
                );
                return Ok(());
            }
            println!("{:<20} KEY", "NAME");
            for entry in entries {
                println!("{:<20} {}", entry.name, entry.public_key);
            }
            Ok(())
        }
        TrustCommand::Add { public_key, name } => {
            host.trust_publisher(&name, &public_key)?;
            println!("已信任 '{name}'");
            Ok(())
        }
        TrustCommand::Remove { name } => {
            if host.untrust_publisher(&name)? {
                println!("已移除 '{name}'");
            } else {
                println!("没有名为 '{name}' 的发布者");
            }
            Ok(())
        }
    })
}
