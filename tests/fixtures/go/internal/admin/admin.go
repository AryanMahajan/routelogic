package admin

import (
	"net/http"

	"github.com/gin-gonic/gin"
)

// Config says where the admin API lives; it comes from the environment at runtime.
type Config struct {
	Prefix string
}

// Register mounts the admin API under the configured prefix.
func Register(rg *gin.RouterGroup, cfg Config) {
	g := rg.Group(cfg.Prefix)
	g.GET("/stats", stats)
	g.POST("/cache/flush", flush)
}

func stats(c *gin.Context) {
	c.JSON(http.StatusOK, gin.H{"users": 0})
}

func flush(c *gin.Context) {
	c.Status(http.StatusAccepted)
}
