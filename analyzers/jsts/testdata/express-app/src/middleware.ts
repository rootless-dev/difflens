import type { NextFunction, Request, Response } from "express";

export function requireUser(req: Request, res: Response, next: NextFunction): void {
  if (req.header("x-user") === undefined) {
    res.status(401).end();
    return;
  }
  next();
}
