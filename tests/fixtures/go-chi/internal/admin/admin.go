package admin

import (
	"encoding/json"
	"net/http"

	"github.com/go-chi/chi/v5"
	"github.com/go-chi/chi/v5/middleware"

	"example.com/notes/internal/store"
)

// Handler serves the admin API.
type Handler struct {
	db *store.DB
}

// NewHandler builds the admin handler.
func NewHandler(db *store.DB) *Handler {
	return &Handler{db: db}
}

// Routes guards everything with basic auth.
func (h *Handler) Routes() chi.Router {
	r := chi.NewRouter()
	r.Use(middleware.BasicAuth("admin", map[string]string{"admin": "secret"}))
	r.Get("/stats", h.stats)
	r.Post("/reindex", h.reindex)
	return r
}

func (h *Handler) stats(w http.ResponseWriter, r *http.Request) {
	json.NewEncoder(w).Encode(map[string]int{"notes": 0})
}

func (h *Handler) reindex(w http.ResponseWriter, r *http.Request) {
	w.WriteHeader(http.StatusAccepted)
}
