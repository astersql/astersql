// Copyright 2026 AsterSQL.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Command etcdhelper starts an embedded etcd for the Rust owner integration
// tests. It keeps the server alive until its parent closes stdin.
package main

import (
	"fmt"
	"io"
	"net/url"
	"os"
	"os/signal"
	"syscall"
	"time"

	"go.etcd.io/etcd/server/v3/embed"
)

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func run() error {
	dir, err := os.MkdirTemp("", "astersql-owner-etcd-")
	if err != nil {
		return fmt.Errorf("create embedded-etcd data directory: %w", err)
	}
	defer os.RemoveAll(dir)

	clientURL, err := url.Parse("http://127.0.0.1:0")
	if err != nil {
		return fmt.Errorf("parse embedded-etcd client URL: %w", err)
	}
	peerURL, err := url.Parse("http://127.0.0.1:0")
	if err != nil {
		return fmt.Errorf("parse embedded-etcd peer URL: %w", err)
	}

	cfg := embed.NewConfig()
	cfg.Dir = dir
	cfg.ListenClientUrls = []url.URL{*clientURL}
	cfg.AdvertiseClientUrls = []url.URL{*clientURL}
	cfg.ListenPeerUrls = []url.URL{*peerURL}
	cfg.AdvertisePeerUrls = []url.URL{*peerURL}
	cfg.InitialCluster = cfg.Name + "=" + peerURL.String()
	cfg.Logger = "zap"
	cfg.LogLevel = "error"

	etcd, err := embed.StartEtcd(cfg)
	if err != nil {
		return fmt.Errorf("start embedded etcd: %w", err)
	}
	defer etcd.Close()

	select {
	case <-etcd.Server.ReadyNotify():
	case <-time.After(10 * time.Second):
		etcd.Server.Stop()
		return fmt.Errorf("embedded etcd did not become ready within 10s")
	}

	fmt.Printf("READY http://%s\n", etcd.Clients[0].Addr().String())

	parentClosed := make(chan struct{})
	go func() {
		_, _ = io.Copy(io.Discard, os.Stdin)
		close(parentClosed)
	}()
	signals := make(chan os.Signal, 1)
	signal.Notify(signals, os.Interrupt, syscall.SIGTERM)
	defer signal.Stop(signals)

	select {
	case <-parentClosed:
	case <-signals:
	case <-etcd.Server.StopNotify():
	}
	return nil
}
