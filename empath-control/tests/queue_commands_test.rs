#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::unreachable
)]

use empath_control::protocol::{
    QueueCommand, QueueListPage, Request, RequestCommand, Response, ResponseData, ResponsePayload,
};

/// Integration test for queue list command via control socket
#[tokio::test]
#[cfg_attr(miri, ignore = "Calls an unsupported method")]
async fn test_queue_list_command() {
    // This test requires a full empath instance with delivery processor
    // For now, we verify the protocol serialization works correctly

    // Test QueueCommand::List serialization
    let request = Request::new(RequestCommand::Queue(QueueCommand::List {
        status_filter: None,
        page: None,
        page_size: None,
    }));

    // Verify serialization/deserialization
    let serialized = postcard::to_allocvec(&request).expect("Failed to serialize request");
    let deserialized = postcard::from_bytes::<Request>(serialized.as_slice())
        .expect("Failed to deserialize request");

    match deserialized.command {
        RequestCommand::Queue(QueueCommand::List {
            status_filter,
            page,
            page_size,
        }) => {
            assert!(status_filter.is_none(), "Expected no status filter");
            assert!(page.is_none());
            assert!(page_size.is_none());
        }
        _ => panic!("Expected QueueCommand::List"),
    }
}

#[tokio::test]
#[cfg_attr(miri, ignore = "Calls an unsupported method")]
async fn test_queue_list_with_status_filter() {
    // Test QueueCommand::List with status filter
    let request = Request::new(RequestCommand::Queue(QueueCommand::List {
        status_filter: Some("failed".to_string()),
        page: Some(2),
        page_size: Some(25),
    }));

    // Verify serialization/deserialization
    let serialized = postcard::to_allocvec(&request).expect("Failed to serialize request");
    let deserialized = postcard::from_bytes::<Request>(serialized.as_slice())
        .expect("Failed to deserialize request");

    match deserialized.command {
        RequestCommand::Queue(QueueCommand::List {
            status_filter,
            page,
            page_size,
        }) => {
            assert_eq!(status_filter.as_deref(), Some("failed"));
            assert_eq!(page, Some(2));
            assert_eq!(page_size, Some(25));
        }
        _ => panic!("Expected QueueCommand::List"),
    }
}

#[tokio::test]
#[cfg_attr(miri, ignore = "Calls an unsupported method")]
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

    let response = Response::data(ResponseData::QueueList(QueueListPage {
        total: 2,
        page: 1,
        page_size: 50,
        has_more: false,
        messages,
    }));

    // Verify serialization/deserialization
    let serialized = postcard::to_allocvec(&response).expect("Failed to serialize response");
    let deserialized = postcard::from_bytes::<Response>(serialized.as_slice())
        .expect("Failed to deserialize response");

    match deserialized.payload {
        ResponsePayload::Data(data) => match *data {
            ResponseData::QueueList(page) => {
                assert_eq!(page.total, 2);
                assert_eq!(page.page, 1);
                assert_eq!(page.page_size, 50);
                assert!(!page.has_more);
                assert_eq!(page.messages.len(), 2, "Expected 2 messages in response");

                // Verify first message
                assert_eq!(page.messages[0].id, "01JCXYZ123ABC");
                assert_eq!(page.messages[0].from, "sender@example.com");
                assert_eq!(page.messages[0].to.len(), 1);
                assert_eq!(page.messages[0].status, "pending");
                assert_eq!(page.messages[0].attempts, 0);
                assert_eq!(page.messages[0].next_retry, None);

                // Verify second message
                assert_eq!(page.messages[1].id, "01JCXYZ456DEF");
                assert_eq!(page.messages[1].status, "failed");
                assert_eq!(page.messages[1].attempts, 3);
                assert_eq!(page.messages[1].to.len(), 2);
                assert_eq!(page.messages[1].next_retry, Some(1_700_001_000));
            }
            _ => panic!("Expected QueueList response"),
        },
        _ => panic!("Expected Data response"),
    }
}

#[tokio::test]
#[cfg_attr(miri, ignore = "Calls an unsupported method")]
async fn test_queue_stats_command() {
    // Test QueueCommand::Stats serialization
    let request = Request::new(RequestCommand::Queue(QueueCommand::Stats));

    // Verify serialization/deserialization
    let serialized = postcard::to_allocvec(&request).expect("Failed to serialize request");
    let deserialized = postcard::from_bytes::<Request>(serialized.as_slice())
        .expect("Failed to deserialize request");

    match deserialized.command {
        RequestCommand::Queue(QueueCommand::Stats) => {
            // Success
        }
        _ => panic!("Expected QueueCommand::Stats"),
    }
}

#[tokio::test]
#[cfg_attr(miri, ignore = "Calls an unsupported method")]
async fn test_queue_view_command() {
    // Test QueueCommand::View serialization
    let request = Request::new(RequestCommand::Queue(QueueCommand::View {
        message_id: "01JCXYZ123ABC".to_string(),
        include_headers: true,
        include_body_preview: false,
    }));

    // Verify serialization/deserialization
    let serialized = postcard::to_allocvec(&request).expect("Failed to serialize request");
    let deserialized = postcard::from_bytes::<Request>(serialized.as_slice())
        .expect("Failed to deserialize request");

    match deserialized.command {
        RequestCommand::Queue(QueueCommand::View {
            message_id,
            include_headers,
            include_body_preview,
        }) => {
            assert_eq!(message_id, "01JCXYZ123ABC", "Expected correct message ID");
            assert!(include_headers);
            assert!(!include_body_preview);
        }
        _ => panic!("Expected QueueCommand::View"),
    }
}

#[tokio::test]
#[cfg_attr(miri, ignore = "Calls an unsupported method")]
async fn test_queue_delete_command() {
    // Test QueueCommand::Delete serialization
    let request = Request::new(RequestCommand::Queue(QueueCommand::Delete {
        message_id: "01JCXYZ123ABC".to_string(),
    }));

    // Verify serialization/deserialization
    let serialized = postcard::to_allocvec(&request).expect("Failed to serialize request");
    let deserialized = postcard::from_bytes::<Request>(serialized.as_slice())
        .expect("Failed to deserialize request");

    match deserialized.command {
        RequestCommand::Queue(QueueCommand::Delete { message_id }) => {
            assert_eq!(message_id, "01JCXYZ123ABC", "Expected correct message ID");
        }
        _ => panic!("Expected QueueCommand::Delete"),
    }
}

#[tokio::test]
#[cfg_attr(miri, ignore = "Calls an unsupported method")]
async fn test_queue_retry_command() {
    // Test QueueCommand::Retry serialization
    let request = Request::new(RequestCommand::Queue(QueueCommand::Retry {
        message_id: "01JCXYZ123ABC".to_string(),
        force: false,
    }));

    // Verify serialization/deserialization
    let serialized = postcard::to_allocvec(&request).expect("Failed to serialize request");
    let deserialized = postcard::from_bytes::<Request>(serialized.as_slice())
        .expect("Failed to deserialize request");

    match deserialized.command {
        RequestCommand::Queue(QueueCommand::Retry { message_id, force }) => {
            assert_eq!(message_id, "01JCXYZ123ABC", "Expected correct message ID");
            assert!(!force, "Expected force to be false");
        }
        _ => panic!("Expected QueueCommand::Retry"),
    }
}
