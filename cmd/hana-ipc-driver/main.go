package main

import (
	"fmt"
	"os"

	"navop-db-ipc-drivers/internal/drivers/hana"
	"navop-db-ipc-drivers/internal/runner"
)

func main() {
	if err := runner.Run(hana.Spec()); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
