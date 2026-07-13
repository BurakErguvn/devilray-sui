use devilray_sui::daemon::DEFAULT_QUEUE_NAME;
use devilray_sui::queue::{MessageQueueTrait, RedisMessageQueue, replay_dlq};
use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let limit = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(100);
    let queue_name = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| DEFAULT_QUEUE_NAME.to_string());

    let redis_url = match env::var("REDIS_URL") {
        Ok(val) => val,
        Err(_) => {
            eprintln!("Error: REDIS_URL environment variable is required.");
            std::process::exit(1);
        }
    };

    let client = redis::Client::open(redis_url)?;
    let conn = client.get_multiplexed_async_connection().await?;
    let queue = RedisMessageQueue::new(conn);
    let dlq_name = format!("{queue_name}_dlq");

    let before = queue.dlq_len(&dlq_name).await?;
    println!("DLQ `{dlq_name}` depth before replay: {before}");

    let replayed = replay_dlq(&queue, &queue_name, &dlq_name, limit).await?;
    let after = queue.dlq_len(&dlq_name).await?;

    println!("Replayed {replayed} message(s) to `{queue_name}`.");
    println!("DLQ `{dlq_name}` depth after replay: {after}");

    Ok(())
}
