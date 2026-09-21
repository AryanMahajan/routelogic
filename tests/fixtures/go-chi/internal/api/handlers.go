package api

import (
	"encoding/json"
	"net/http"

	"github.com/go-chi/chi/v5"
)

type note struct {
	ID    string   `json:"id"`
	Title string   `json:"title"`
	Body  string   `json:"body"`
	Tags  []string `json:"tags,omitempty"`
}

type createNote struct {
	Title string   `json:"title" validate:"required"`
	Body  string   `json:"body"`
	Tags  []string `json:"tags"`
}

func (s *Server) health(w http.ResponseWriter, r *http.Request) {
	w.WriteHeader(http.StatusOK)
}

func (s *Server) listNotes(w http.ResponseWriter, r *http.Request) {
	q := r.URL.Query()
	tag := q.Get("tag")
	page := r.URL.Query().Get("page")
	_, _ = tag, page
	json.NewEncoder(w).Encode([]note{})
}

func (s *Server) createNote(w http.ResponseWriter, r *http.Request) {
	var req createNote
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	w.WriteHeader(http.StatusCreated)
}

func (s *Server) getNote(w http.ResponseWriter, r *http.Request) {
	id := chi.URLParam(r, "id")
	json.NewEncoder(w).Encode(note{ID: id})
}

func (s *Server) updateNote(w http.ResponseWriter, r *http.Request) {
	var req note
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (s *Server) deleteNote(w http.ResponseWriter, r *http.Request) {
	if r.Header.Get("X-Confirm") != "yes" {
		http.Error(w, "confirm", http.StatusPreconditionRequired)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}
