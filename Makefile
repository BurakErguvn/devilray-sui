check:
	cargo check

test:
	cargo test

fmt:
	cargo fmt

clippy:
	cargo clippy --all-targets

lint:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings

test-integration:
	docker compose -f docker-compose.test.yml up -d --wait test-postgres test-redis
	DATABASE_URL=postgres://test_user:test_password@localhost:5433/test_db \
	REDIS_URL=redis://localhost:6379 \
	cargo test --features integration

ci: lint test

run:
	cargo run

docker-up:
	docker compose up --build

docker-down:
	docker compose down
