package main

import (
	"log"
	"net/http"

	"example.com/notes/internal/api"
	"example.com/notes/internal/store"
)

func main() {
	db := store.Open("notes.db")
	srv := api.NewServer(db)
	log.Fatal(http.ListenAndServe(":3000", srv))
}
