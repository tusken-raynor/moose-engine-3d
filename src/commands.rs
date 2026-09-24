use std::collections::HashMap;

use crate::{console, ss::set_save_frame_buffer};

/**
 * In this file we will define all the commands that we want to use in our CLI
 * as well as the controller struct the receives the commands and executes them.
 */

pub struct CommandExecutor {
  command_index_map: HashMap<String, usize>,
}

 // Define the methods for the truct here
impl CommandExecutor {
  // Define the constructor for the struct here
  pub fn new() -> CommandExecutor {
    // Generate the command index map
    let mut command_index_map: HashMap<String, usize> = HashMap::new();
    for (i, command) in COMMANDS.iter().enumerate() {
      command_index_map.insert(command.name.to_string(), i);
    }
    CommandExecutor {
      command_index_map
    }
  }
  
    // Define the methods for the struct here
  pub fn execute(&self, command: String) {
    let command = command.trim();
    // Check if the command is empty
    if command.is_empty() {
      // If it is, then return
      return;
    }
  
    // Split the command into a vector of strings
    let parts: Vec<String> = command.split(" ").map(|s| s.to_string()).collect();
    let command = parts[0].clone();
    let args = parts[1..].to_vec();
    // Check the hashmap to see if the command exists
    let command_index = self.command_index_map.get(&command);
    match command_index {
      Some(index) => {
        // If it does, then execute the command
        let command = &COMMANDS[*index];
        let output = (command.function)(&args);
        if !output.is_empty() {
          console::log(output);
        }
      },
      None => {
        // If it doesn't, then print an error message
        console::log(format!("Command \"{}\" not found. Type help for a list of available commands.", command));
      }
    }
  }
}

type CommandFunction = fn(&Vec<String>) -> String;

pub struct Command {
  pub name: &'static str,
  pub description: &'static str,
  pub function: CommandFunction,
}

/**
 * DEFINE HERE THE FIXED COMMANDS THAT YOU WANT TO USE IN YOUR CLI
 */
const COMMAND_COUNT: usize = 7;
const COMMANDS: [Command; COMMAND_COUNT] = [
  Command {
    name: "help",
    description: "Prints a list of available commands, and their descriptions",
    function: help
  },
  Command {
    name: "exit",
    description: "Exits the Command Line Interface",
    function: exit
  },
  Command {
    name: "quit",
    description: "Exits the program",
    function: quit
  },
  Command {
    name: "echo",
    description: "Prints the provided arguments",
    function: echo
  },
  Command {
    name: "restart",
    description: "Restarts the program",
    function: restart
  },
  Command {
    name: "clear",
    description: "Clear the output from the console",
    function: clear
  },
  Command {
    name: "ss",
    description: "Save a screenshot of the current frame",
    function: ss
  },
];

fn help(_: &Vec<String>) -> String {
  // Loop through the commands and add the to a list alphanumerically
  let mut command_list: Vec<String> = vec![];
  for command in COMMANDS.iter() {
    command_list.push(format!("{} - {}", command.name, command.description));
  }
  // Sort the list
  command_list.sort();
  // Add the message to the beginning of the list
  command_list.insert(0, String::from("\nAvailable commands:"));
  // Return the list
  command_list.join("\n")
}

fn exit(_: &Vec<String>) -> String {
  // Perform any necessary cleanup or actions before exiting
  console::close();
  String::from("")
}

fn quit(_: &Vec<String>) -> String {
  // Exit the program
  std::process::exit(0);
}

fn echo(args: &Vec<String>) -> String {
  // Print the arguments
  args.join(" ")
}

fn restart(_: &Vec<String>) -> String {
  // Restart the program
  std::process::Command::new("cargo")
    .args(&["run", "--release"])
    .spawn()
    .expect("Failed to restart the program");
  // Exit the program
  std::process::exit(0);
}

fn clear(_: &Vec<String>) -> String {
  // Clear the console
  console::clear();
  String::from("")
}

fn ss(_: &Vec<String>) -> String {
  // Save a screenshot of the current frame
  set_save_frame_buffer();
  String::from("Screenshot captured!")
}