package main

import (
	"log"

	"github.com/yourpovv/viewpoint/server/internal/config"
	serving "github.com/yourpovv/viewpoint/server/internal/http"
	"github.com/yourpovv/viewpoint/server/internal/signal"
)

func main() {
	if err := run(); err != nil {
		log.Fatal(err)
	}
}

func run() error {
	cfg, err := config.Load("viewpoint.yaml")
	if err != nil {
		return err
	}
	links := signal.NewRegistry()
	server := serving.NewServer(cfg, links, "web")
	log.Printf("Viewpoint signaling on %s:%d", cfg.Server.Host, cfg.Server.Port)
	return server.Start()
}
