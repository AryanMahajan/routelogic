package legacy

import "net/http"

// Mux was the first API. Nothing serves it any more.
var Mux = http.NewServeMux()

func init() {
	Mux.HandleFunc("/v0/notes", func(w http.ResponseWriter, r *http.Request) {
		http.Error(w, "gone", http.StatusGone)
	})
}
