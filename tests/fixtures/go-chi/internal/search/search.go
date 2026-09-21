package search

import (
	"encoding/json"
	"net/http"

	"github.com/go-chi/chi/v5"
)

var table = map[string]http.HandlerFunc{
	"/":        query,
	"/recent":  recent,
}

// Routes registers the search API from a table.
func Routes() chi.Router {
	r := chi.NewRouter()
	for pattern, handler := range table {
		r.Get(pattern, handler)
	}
	return r
}

func query(w http.ResponseWriter, r *http.Request) {
	json.NewEncoder(w).Encode([]string{})
}

func recent(w http.ResponseWriter, r *http.Request) {
	json.NewEncoder(w).Encode([]string{})
}
