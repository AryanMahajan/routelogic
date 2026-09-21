package v2

import (
	"encoding/json"
	"net/http"
)

// Mux is the v2 API on the standard library's router, with Go 1.22 method patterns.
func Mux() *http.ServeMux {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /notes/{id}", getNote)
	mux.HandleFunc("POST /notes", createNote)
	mux.HandleFunc("DELETE /notes/{id}", deleteNote)
	mux.HandleFunc("GET /files/{path...}", file)
	mux.HandleFunc("/export", export)
	mux.HandleFunc("/{$}", index)
	return mux
}

type noteV2 struct {
	Title string `json:"title"`
}

func getNote(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	json.NewEncoder(w).Encode(map[string]string{"id": id})
}

func createNote(w http.ResponseWriter, r *http.Request) {
	var n noteV2
	if err := json.NewDecoder(r.Body).Decode(&n); err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	w.WriteHeader(http.StatusCreated)
}

func deleteNote(w http.ResponseWriter, r *http.Request) {
	w.WriteHeader(http.StatusNoContent)
}

func file(w http.ResponseWriter, r *http.Request) {
	w.WriteHeader(http.StatusOK)
}

func export(w http.ResponseWriter, r *http.Request) {
	switch r.Method {
	case http.MethodGet:
		format := r.URL.Query().Get("format")
		_ = format
	case http.MethodPost:
		w.WriteHeader(http.StatusAccepted)
	default:
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
	}
}

func index(w http.ResponseWriter, r *http.Request) {
	w.WriteHeader(http.StatusOK)
}
