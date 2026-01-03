use std::net::SocketAddr;
use std::sync::Arc;

use empath_common::address::{Address, AddressList};
use empath_common::address_parser::Mailbox;
use empath_common::context::Context;
use empath_common::envelope::Envelope;
use empath_smtp_client::{
    CertValidation, SmtpClient, SmtpTimeouts, SmtpTlsConfig, SmtpTransaction, TlsMode,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FinalCommand {
    Quit,
    Rset,
}

async fn spawn_mock_server(
    final_command: FinalCommand,
) -> Result<
    (
        SocketAddr,
        tokio::task::JoinHandle<Result<Vec<String>, String>>,
    ),
    String,
> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|err| err.to_string())?;
    let addr = listener.local_addr().map_err(|err| err.to_string())?;
    let handle = tokio::spawn(async move { handle_client(listener, final_command).await });
    Ok((addr, handle))
}

async fn handle_client(
    listener: TcpListener,
    final_command: FinalCommand,
) -> Result<Vec<String>, String> {
    let (stream, _) = listener.accept().await.map_err(|err| err.to_string())?;
    let (read, mut write) = stream.into_split();
    write
        .write_all(b"220 mock.local ESMTP\r\n")
        .await
        .map_err(|err| err.to_string())?;

    let mut reader = BufReader::new(read);
    let mut commands = Vec::new();
    let mut in_data = false;

    loop {
        let mut line = String::new();
        let bytes = reader
            .read_line(&mut line)
            .await
            .map_err(|err| err.to_string())?;
        if bytes == 0 {
            return Err("connection closed".to_string());
        }

        let trimmed = line.trim_end_matches(&['\r', '\n'][..]);
        if trimmed.is_empty() {
            continue;
        }

        if in_data {
            if trimmed == "." {
                in_data = false;
                write
                    .write_all(b"250 OK\r\n")
                    .await
                    .map_err(|err| err.to_string())?;
            }
            continue;
        }

        commands.push(trimmed.to_string());

        if trimmed.starts_with("EHLO ") {
            write
                .write_all(b"250-mock.local\r\n250 STARTTLS\r\n")
                .await
                .map_err(|err| err.to_string())?;
            continue;
        }

        if trimmed.starts_with("MAIL FROM:") || trimmed.starts_with("RCPT TO:") {
            write
                .write_all(b"250 OK\r\n")
                .await
                .map_err(|err| err.to_string())?;
            continue;
        }

        if trimmed == "DATA" {
            in_data = true;
            write
                .write_all(b"354 End data with <CR><LF>.<CR><LF>\r\n")
                .await
                .map_err(|err| err.to_string())?;
            continue;
        }

        if trimmed == "QUIT" {
            if final_command != FinalCommand::Quit {
                return Err("unexpected QUIT".to_string());
            }
            write
                .write_all(b"221 Bye\r\n")
                .await
                .map_err(|err| err.to_string())?;
            break;
        }

        if trimmed == "RSET" {
            if final_command != FinalCommand::Rset {
                return Err("unexpected RSET".to_string());
            }
            write
                .write_all(b"250 OK\r\n")
                .await
                .map_err(|err| err.to_string())?;
            break;
        }

        return Err(format!("unexpected command: {trimmed}"));
    }

    Ok(commands)
}

fn make_context() -> Context {
    let mut envelope = Envelope::default();
    let sender = Address::from(Mailbox {
        local_part: "sender".to_string(),
        domain: "example.com".to_string(),
    });
    let recipient = Address::from(Mailbox {
        local_part: "rcpt".to_string(),
        domain: "example.org".to_string(),
    });
    *envelope.sender_mut() = Some(sender);
    *envelope.recipients_mut() = Some(AddressList::from(vec![recipient]));

    let data: Arc<[u8]> = Arc::from(b"Subject: test\r\n\r\nbody\r\n".as_ref());
    Context {
        envelope: Arc::new(envelope),
        id: "example.com".to_string(),
        data: Some(data.clone()),
        data_len: Some(data.len()),
        ..Context::default()
    }
}

#[tokio::test]
#[cfg_attr(
    miri,
    ignore = "Miri does not support kqueue-backed Tokio runtime on macOS"
)]
async fn transaction_execute_sends_quit() -> Result<(), String> {
    let (addr, handle) = spawn_mock_server(FinalCommand::Quit).await?;
    let context = make_context();
    let timeouts = SmtpTimeouts::default();
    let tls_config = SmtpTlsConfig::new(TlsMode::Disabled, CertValidation::Strict);
    let transaction = SmtpTransaction::new(&context, addr.to_string(), tls_config, &timeouts);

    transaction.execute().await.map_err(|err| err.to_string())?;

    let commands = handle.await.map_err(|err| err.to_string())??;
    if commands.last().map(String::as_str) != Some("QUIT") {
        return Err("expected QUIT after transaction".to_string());
    }
    if commands.first().map(String::as_str) != Some("EHLO example.com") {
        return Err("expected EHLO with context domain".to_string());
    }

    Ok(())
}

#[tokio::test]
#[cfg_attr(
    miri,
    ignore = "Miri does not support kqueue-backed Tokio runtime on macOS"
)]
async fn transaction_execute_with_client_sends_rset() -> Result<(), String> {
    let (addr, handle) = spawn_mock_server(FinalCommand::Rset).await?;
    let context = make_context();
    let timeouts = SmtpTimeouts::default();
    let tls_config = SmtpTlsConfig::new(TlsMode::Disabled, CertValidation::Strict);
    let transaction = SmtpTransaction::new(&context, addr.to_string(), tls_config, &timeouts);

    let mut client = SmtpClient::connect(&addr.to_string(), "localhost".to_string())
        .await
        .map_err(|err| err.to_string())?;
    let greeting = client
        .read_greeting()
        .await
        .map_err(|err| err.to_string())?;
    if !greeting.is_success() {
        return Err("unexpected greeting failure".to_string());
    }

    let pooled = transaction
        .execute_with_client(client)
        .await
        .map_err(|err| err.to_string())?;
    if pooled.is_none() {
        return Err("expected pooled client to be retained after RSET".to_string());
    }

    let commands = handle.await.map_err(|err| err.to_string())??;
    if commands.last().map(String::as_str) != Some("RSET") {
        return Err("expected RSET after pooled transaction".to_string());
    }

    Ok(())
}
