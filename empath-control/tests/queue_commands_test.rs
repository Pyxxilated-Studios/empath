#![allow(clippy::expect_used, clippy::unwrap_used)]

use empath_control::protocol::{
    QueueCommand, Request, RequestCommand, Response, ResponseData, ResponsePayload,
};

/// Integration test for queue list command via control socket
#[tokio::test]
async fn test_queue_list_command() {
    // This test requires a full empath instance with delivery processor
    // For now, we verify the protocol serialization works correctly

    // Test QueueCommand::List serialization
    let request = Request::new(RequestCommand::Queue(QueueCommand::List {
        status_filter: None,
    }));

    // Verify serialization/deserialization
    let serialized = bincode::serde::encode_to_vec(request, bincode::config::legacy())
        .expect("Failed to serialize request");
    let (deserialized, _): (Request, _) =
        bincode::serde::decode_from_slice(serialized.as_slice(), bincode::config::legacy())
            .expect("Failed to deserialize request");

    match deserialized.command {
        RequestCommand::Queue(QueueCommand::List { status_filter }) => {
            assert!(status_filter.is_none(), "Expected no status filter");
        }
        _ => panic!("Expected QueueCommand::List"),
    }
}

#[tokio::test]
async fn test_queue_list_with_status_filter() {
    // Test QueueCommand::List with status filter
    let request = Request::new(RequestCommand::Queue(QueueCommand::List {
        status_filter: Some("failed".to_string()),
    }));

    // Verify serialization/deserialization
    let serialized = bincode::serde::encode_to_vec(request, bincode::config::legacy())
        .expect("Failed to serialize request");
    let (deserialized, _): (Request, _) =
        bincode::serde::decode_from_slice(serialized.as_slice(), bincode::config::legacy())
            .expect("Failed to deserialize request");

    match deserialized.command {
        RequestCommand::Queue(QueueCommand::List { status_filter }) => {
            assert_eq!(
                status_filter,
                Some("failed".to_string()),
                "Expected 'failed' status filter"
            );
        }
        _ => panic!("Expected QueueCommand::List"),
    }
}

#[tokio::test]
async fn test_queue_message_response_serialization() {
    use empath_control::protocol::QueueMessage;

    // Test response serialization
    let messages = vec![
        QueueMessage {
            id: "01JCXYZ123ABC".to_string(),
            from: "sender@example.com".to_string(),
            to: vec!["recipient@example.com".to_string()],
            domain: "example.com".to_string(),
            status: "pending".to_string(),
            attempts: 0,
            next_retry: None,
            size: 1024,
            spooled_at: 1_700_000_000,
        },
        QueueMessage {
            id: "01JCXYZ456DEF".to_string(),
            from: "another@example.com".to_string(),
            to: vec!["user1@test.com".to_string(), "user2@test.com".to_string()],
            domain: "test.com".to_string(),
            status: "failed".to_string(),
            attempts: 3,
            next_retry: Some(1_700_001_000),
            size: 2048,
            spooled_at: 1_700_000_500,
        },
    ];

    let response = Response::data(ResponseData::QueueList(messages));

    // Verify serialization/deserialization
    let serialized = bincode::serde::encode_to_vec(response, bincode::config::legacy())
        .expect("Failed to serialize response");
    let (deserialized, _): (Response, _) =
        bincode::serde::decode_from_slice(serialized.as_slice(), bincode::config::legacy())
            .expect("Failed to deserialize response");

    match deserialized.payload {
        ResponsePayload::Data(data) => match *data {
            ResponseData::QueueList(deserialized_messages) => {
                assert_eq!(
                    deserialized_messages.len(),
                    2,
                    "Expected 2 messages in response"
                );

                // Verify first message
                assert_eq!(deserialized_messages[0].id, "01JCXYZ123ABC");
                assert_eq!(deserialized_messages[0].from, "sender@example.com");
                assert_eq!(deserialized_messages[0].to.len(), 1);
                assert_eq!(deserialized_messages[0].status, "pending");
                assert_eq!(deserialized_messages[0].attempts, 0);
                assert_eq!(deserialized_messages[0].next_retry, None);

                // Verify second message
                assert_eq!(deserialized_messages[1].id, "01JCXYZ456DEF");
                assert_eq!(deserialized_messages[1].status, "failed");
                assert_eq!(deserialized_messages[1].attempts, 3);
                assert_eq!(deserialized_messages[1].to.len(), 2);
                assert_eq!(deserialized_messages[1].next_retry, Some(1_700_001_000));
            }
            _ => panic!("Expected QueueList response"),
        },
        _ => panic!("Expected Data response"),
    }
}

#[tokio::test]
async fn test_queue_stats_command() {
    // Test QueueCommand::Stats serialization
    let request = Request::new(RequestCommand::Queue(QueueCommand::Stats));

    // Verify serialization/deserialization
    let serialized = bincode::serde::encode_to_vec(request, bincode::config::legacy())
        .expect("Failed to serialize request");
    let (deserialized, _): (Request, _) =
        bincode::serde::decode_from_slice(serialized.as_slice(), bincode::config::legacy())
            .expect("Failed to deserialize request");

    match deserialized.command {
        RequestCommand::Queue(QueueCommand::Stats) => {
            // Success
        }
        _ => panic!("Expected QueueCommand::Stats"),
    }
}

#[tokio::test]
async fn test_queue_view_command() {
    // Test QueueCommand::View serialization
    let request = Request::new(RequestCommand::Queue(QueueCommand::View {
        message_id: "01JCXYZ123ABC".to_string(),
    }));

    // Verify serialization/deserialization
    let serialized = bincode::serde::encode_to_vec(request, bincode::config::legacy())
        .expect("Failed to serialize request");
    let (deserialized, _): (Request, _) =
        bincode::serde::decode_from_slice(serialized.as_slice(), bincode::config::legacy())
            .expect("Failed to deserialize request");

    match deserialized.command {
        RequestCommand::Queue(QueueCommand::View { message_id }) => {
            assert_eq!(message_id, "01JCXYZ123ABC", "Expected correct message ID");
        }
        _ => panic!("Expected QueueCommand::View"),
    }
}

#[tokio::test]
async fn test_queue_delete_command() {
    // Test QueueCommand::Delete serialization
    let request = Request::new(RequestCommand::Queue(QueueCommand::Delete {
        message_id: "01JCXYZ123ABC".to_string(),
    }));

    // Verify serialization/deserialization
    let serialized = bincode::serde::encode_to_vec(request, bincode::config::legacy())
        .expect("Failed to serialize request");
    let (deserialized, _): (Request, _) =
        bincode::serde::decode_from_slice(serialized.as_slice(), bincode::config::legacy())
            .expect("Failed to deserialize request");

    match deserialized.command {
        RequestCommand::Queue(QueueCommand::Delete { message_id }) => {
            assert_eq!(message_id, "01JCXYZ123ABC", "Expected correct message ID");
        }
        _ => panic!("Expected QueueCommand::Delete"),
    }
}

#[tokio::test]
async fn test_queue_retry_command() {
    // Test QueueCommand::Retry serialization
    let request = Request::new(RequestCommand::Queue(QueueCommand::Retry {
        message_id: "01JCXYZ123ABC".to_string(),
        force: false,
    }));

    // Verify serialization/deserialization
    let serialized = bincode::serde::encode_to_vec(request, bincode::config::legacy())
        .expect("Failed to serialize request");
    let (deserialized, _): (Request, _) =
        bincode::serde::decode_from_slice(serialized.as_slice(), bincode::config::legacy())
            .expect("Failed to deserialize request");

    match deserialized.command {
        RequestCommand::Queue(QueueCommand::Retry { message_id, force }) => {
            assert_eq!(message_id, "01JCXYZ123ABC", "Expected correct message ID");
            assert!(!force, "Expected force to be false");
        }
        _ => panic!("Expected QueueCommand::Retry"),
    }
}
