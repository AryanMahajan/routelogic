package api

import (
	"net/http"
	"os"

	"github.com/go-chi/chi/v5"
	"github.com/go-chi/chi/v5/middleware"
	"github.com/go-chi/jwtauth/v5"

	"example.com/notes/internal/admin"
	"example.com/notes/internal/search"
	"example.com/notes/internal/store"
	"example.com/notes/internal/tags"
	"example.com/notes/internal/v2"
)

var tokenAuth = jwtauth.New("HS256", []byte("secret"), nil)

// Server owns the router and serves it.
type Server struct {
	db     *store.DB
	router chi.Router
}

// NewServer builds the server and its routes.
func NewServer(db *store.DB) *Server {
	s := &Server{db: db, router: chi.NewRouter()}
	s.routes()
	return s
}

// ServeHTTP hands every request to the router.
func (s *Server) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	s.router.ServeHTTP(w, r)
}

func (s *Server) routes() {
	s.router.Use(middleware.Logger)
	s.router.Use(middleware.Recoverer)

	s.router.Get("/health", s.health)

	s.router.Route("/notes", func(r chi.Router) {
		r.Get("/", s.listNotes)
		r.With(jwtauth.Verifier(tokenAuth), jwtauth.Authenticator(tokenAuth)).Post("/", s.createNote)
		r.Route("/{id}", func(r chi.Router) {
			r.Use(jwtauth.Verifier(tokenAuth))
			r.Get("/", s.getNote)
			r.Put("/", s.updateNote)
			r.Delete("/", s.deleteNote)
		})
	})

	s.router.Mount("/tags", tags.Routes())
	s.router.Mount("/labels", tags.Routes())
	s.router.Mount(os.Getenv("ADMIN_PREFIX"), admin.NewHandler(s.db).Routes())
	s.router.Mount("/search", search.Routes())

	s.router.Handle("/v2/", http.StripPrefix("/v2", v2.Mux()))
	s.router.Handle("/static/*", http.StripPrefix("/static/", http.FileServer(http.Dir("public"))))
}
