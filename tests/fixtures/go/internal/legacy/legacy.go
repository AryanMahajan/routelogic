package legacy

import (
	"net/http"

	"github.com/gin-gonic/gin"
)

// Register mounts the v0 API. Nothing calls it any more.
func Register(rg *gin.RouterGroup) {
	rg.GET("/v0/users", func(c *gin.Context) {
		c.JSON(http.StatusGone, gin.H{"error": "gone"})
	})
}
