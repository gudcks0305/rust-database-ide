//! Synthetic PostgreSQL wire tests. No network or database credentials required.
use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinHandle;
use tokio_postgres::{Client, Config, NoTls, SimpleQueryMessage, TransactionStatus};

fn frame(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut bytes = vec![tag];
    bytes.extend_from_slice(&((body.len() + 4) as u32).to_be_bytes());
    bytes.extend_from_slice(body);
    bytes
}

async fn client_with_response(response: Vec<u8>) -> (Client, JoinHandle<()>, JoinHandle<()>) {
    let (socket, mut server) = tokio::io::duplex(4096);
    let server_task = tokio::spawn(async move {
        let startup_len = server.read_u32().await.unwrap();
        let mut startup = vec![0; startup_len as usize - 4];
        server.read_exact(&mut startup).await.unwrap();
        server.write_all(&frame(b'R', &[0, 0, 0, 0])).await.unwrap();
        server.write_all(&frame(b'Z', b"I")).await.unwrap();
        assert_eq!(server.read_u8().await.unwrap(), b'Q');
        let query_len = server.read_u32().await.unwrap();
        let mut query = vec![0; query_len as usize - 4];
        server.read_exact(&mut query).await.unwrap();
        server.write_all(&response).await.unwrap();
        // Closing here also tests that missing ReadyForQuery never invents state.
    });
    let mut config = Config::new();
    config.user("synthetic_wire_test");
    let (client, connection) = config.connect_raw(socket, NoTls).await.unwrap();
    let connection_task = tokio::spawn(async move {
        // Synthetic peers deliberately close immediately after their response.
        let _ = connection.await;
    });
    (client, connection_task, server_task)
}

#[tokio::test]
async fn status_remains_unknown_until_ready_and_emits_no_extra_item() {
    for (wire_status, expected) in [
        (b'I', TransactionStatus::Idle),
        (b'T', TransactionStatus::InTransaction),
        (b'E', TransactionStatus::InError),
    ] {
        let mut response = frame(b'C', b"SELECT 1\0");
        response.extend(frame(b'Z', &[wire_status]));
        let (client, connection, server) = client_with_response(response).await;
        let stream = client.simple_query_raw("synthetic request").await.unwrap();
        tokio::pin!(stream);
        assert_eq!(stream.as_ref().get_ref().transaction_status(), None);
        assert!(matches!(
            stream.next().await,
            Some(Ok(SimpleQueryMessage::CommandComplete(1)))
        ));
        assert_eq!(stream.as_ref().get_ref().transaction_status(), None);
        assert!(stream.next().await.is_none());
        assert_eq!(
            stream.as_ref().get_ref().transaction_status(),
            Some(expected)
        );
        server.await.unwrap();
        connection.abort();
    }
}

#[tokio::test]
async fn database_error_can_be_drained_to_failed_transaction_status() {
    let mut response = frame(b'E', b"SERROR\0C22012\0Mdivision by zero\0\0");
    response.extend(frame(b'Z', b"E"));
    let (client, connection, server) = client_with_response(response).await;
    let stream = client.simple_query_raw("synthetic request").await.unwrap();
    tokio::pin!(stream);
    let error = stream.next().await.unwrap().unwrap_err();
    assert_eq!(error.as_db_error().unwrap().code().code(), "22012");
    assert_eq!(stream.as_ref().get_ref().transaction_status(), None);
    assert!(stream.next().await.is_none());
    assert_eq!(
        stream.as_ref().get_ref().transaction_status(),
        Some(TransactionStatus::InError)
    );
    server.await.unwrap();
    connection.abort();
}

#[tokio::test]
async fn connection_failure_before_ready_leaves_unknown_status() {
    let (client, connection, server) = client_with_response(frame(b'C', b"SELECT 1\0")).await;
    let stream = client.simple_query_raw("synthetic request").await.unwrap();
    tokio::pin!(stream);
    assert!(stream.next().await.unwrap().is_ok());
    assert!(stream.next().await.unwrap().is_err());
    assert_eq!(stream.as_ref().get_ref().transaction_status(), None);
    server.await.unwrap();
    connection.abort();
}

#[tokio::test]
async fn every_invalid_status_byte_is_rejected_without_fabricating_state() {
    for status in u8::MIN..=u8::MAX {
        if matches!(status, b'I' | b'T' | b'E') {
            continue;
        }
        let (client, connection, server) = client_with_response(frame(b'Z', &[status])).await;
        let stream = client.simple_query_raw("synthetic request").await.unwrap();
        tokio::pin!(stream);
        assert!(stream.next().await.unwrap().is_err(), "wire byte {status}");
        assert_eq!(stream.as_ref().get_ref().transaction_status(), None);
        server.await.unwrap();
        connection.abort();
    }
}

#[tokio::test]
async fn collected_simple_query_preserves_existing_message_shape() {
    let mut response = frame(b'C', b"SELECT 1\0");
    response.extend(frame(b'Z', b"I"));
    let (client, connection, server) = client_with_response(response).await;
    let messages = client.simple_query("synthetic request").await.unwrap();
    assert!(matches!(
        messages.as_slice(),
        [SimpleQueryMessage::CommandComplete(1)]
    ));
    server.await.unwrap();
    connection.abort();
}
