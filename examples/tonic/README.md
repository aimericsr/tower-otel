## Sending the request to the server
curl -X POST -v "http://localhost:3000/api/v1/hello/jean?page=1&per_page=20"

## Lunch the server
RUST_LOG="info" OTEL_RESOURCE_ATTRIBUTES="service.name=bonjour, environment=staging" cargo run 