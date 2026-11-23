// NestJS is not installed in this fixture: the decorators are recognised by name.
import { Controller, Get, Post, Param } from "@nestjs/common";
import { EventPattern } from "@nestjs/microservices";
import { Cron } from "@nestjs/schedule";

@Controller("orders")
export class OrdersController {
  @Get(":id")
  findOne(@Param("id") id: string): string {
    return this.format(id);
  }

  @Post()
  create(): string {
    return "created";
  }

  @EventPattern("order.paid")
  onPaid(): void {}

  @Cron("0 * * * *")
  sweep(): void {}

  private format(id: string): string {
    return `order ${id}`;
  }
}
