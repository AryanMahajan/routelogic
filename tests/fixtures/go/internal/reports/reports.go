package reports

import (
	"net/http"

	"github.com/gin-gonic/gin"
)

var periods = []string{"/daily", "/weekly"}

// Register mounts the reports API. The routes are built at runtime, which static
// analysis can only partly see.
func Register(rg *gin.RouterGroup) {
	g := rg.Group("/reports")
	for _, period := range periods {
		g.GET(period, report)
	}
	method := "GET"
	g.Handle(method, "/summary", summary)
}

func report(c *gin.Context) {
	c.JSON(http.StatusOK, gin.H{})
}

func summary(c *gin.Context) {
	c.JSON(http.StatusOK, gin.H{})
}
