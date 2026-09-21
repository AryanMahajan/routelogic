package tags

import (
	"encoding/json"
	"net/http"

	"github.com/go-chi/chi/v5"
)

// Routes lists and reads tags.
func Routes() chi.Router {
	r := chi.NewRouter()
	r.Get("/", list)
	r.Get("/{name}", get)
	r.Get("/{name}/notes", notes)
	return r
}

func list(w http.ResponseWriter, r *http.Request) {
	json.NewEncoder(w).Encode([]string{})
}

func get(w http.ResponseWriter, r *http.Request) {
	json.NewEncoder(w).Encode(chi.URLParam(r, "name"))
}

func notes(w http.ResponseWriter, r *http.Request) {
	limit := r.URL.Query().Get("limit")
	_ = limit
	json.NewEncoder(w).Encode([]string{})
}
