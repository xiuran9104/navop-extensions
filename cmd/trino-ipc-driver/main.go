package main

import (
	"fmt"
	"os"

	"navop-db-ipc-drivers/internal/drivers/trino"
	"navop-db-ipc-drivers/internal/runner"
)

func main() {
	if err := runner.Run(trino.Spec()); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
