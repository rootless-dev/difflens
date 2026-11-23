#!/usr/bin/env node
const { Command } = require("commander");

const { greet, shout } = require("../lib/greet");

const program = new Command();
program.name("democli");

program
  .command("greet <name>")
  .description("say hello")
  .action((name) => {
    console.log(greet(name));
  });

program.command("shout <name>").action(shoutCommand);

function shoutCommand(name) {
  console.log(shout(name));
}

program.parse(process.argv);
