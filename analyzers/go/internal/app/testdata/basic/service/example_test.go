package service_test

import "example.com/basic/service"

func ExampleService_Request() {
	_ = service.NewService(false).Request("k")
}
