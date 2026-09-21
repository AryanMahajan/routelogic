FROM golang:1.22
WORKDIR /app
COPY . .
RUN go build -o server ./cmd/server
EXPOSE 8080
CMD ["./server"]
