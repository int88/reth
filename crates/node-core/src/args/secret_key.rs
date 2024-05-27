use reth_network::config::rng_secret_key;
use reth_primitives::{fs, fs::FsPathError, hex::encode as hex_encode};
use secp256k1::{Error as SecretKeyBaseError, SecretKey};
use std::{
    io,
    path::{Path, PathBuf},
};
use thiserror::Error;

/// Errors returned by loading a [`SecretKey`], including IO errors.
/// 加载一个[`SecretKey`]的错误，包含IO错误
#[derive(Error, Debug)]
pub enum SecretKeyError {
    /// Error encountered during decoding of the secret key.
    /// 在解码secret key的时候遇到的错误
    #[error(transparent)]
    SecretKeyDecodeError(#[from] SecretKeyBaseError),

    /// Error related to file system path operations.
    /// 文件系统路径操作遇到的错误
    #[error(transparent)]
    SecretKeyFsPathError(#[from] FsPathError),

    /// Represents an error when failed to access the key file.
    /// 当访问key文件时遇到的错误
    #[error("failed to access key file {secret_file:?}: {error}")]
    FailedToAccessKeyFile {
        /// The encountered IO error.
        /// 遇到的IO错误
        error: io::Error,
        /// Path to the secret key file.
        /// 到secret key文件的路径
        secret_file: PathBuf,
    },
}

/// Attempts to load a [`SecretKey`] from a specified path. If no file exists there, then it
/// generates a secret key and stores it in the provided path. I/O errors might occur during write
/// operations in the form of a [`SecretKeyError`]
/// 试着加载一个[`SecretKey`]从特定的路径，如果没有文件存在，那么生成一个secret
/// key并且存在提供的路径，I/O错误可能发生，在写入操作的时候，以 [`SecretKeyError`]的形式
pub fn get_secret_key(secret_key_path: &Path) -> Result<SecretKey, SecretKeyError> {
    let exists = secret_key_path.try_exists();

    match exists {
        Ok(true) => {
            let contents = fs::read_to_string(secret_key_path)?;
            Ok(contents
                .as_str()
                .parse::<SecretKey>()
                .map_err(SecretKeyError::SecretKeyDecodeError)?)
        }
        Ok(false) => {
            if let Some(dir) = secret_key_path.parent() {
                // Create parent directory
                // 创建parent目录
                fs::create_dir_all(dir)?;
            }

            // 生成secret key
            let secret = rng_secret_key();
            // 编码
            let hex = hex_encode(secret.as_ref());
            fs::write(secret_key_path, hex)?;
            Ok(secret)
        }
        Err(error) => Err(SecretKeyError::FailedToAccessKeyFile {
            error,
            secret_file: secret_key_path.to_path_buf(),
        }),
    }
}
