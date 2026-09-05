mod agent;
mod model;
mod tools;
mod types;

use anyhow::{Context, Result};
use agent::Agent;
use dotenvy::dotenv;
use model::openrouter::OpenRouterClient;
use std::{
    env,
    io::{self, Write},
};
use tools::{
    shell::ShellTool,
    process::ProcessTool,
    ToolRegistry,
};


#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    let api_key =
        env::var("OPENROUTER_API_KEY")
            .context(
                "OPENROUTER_API_KEY is missing"
            )?;

    let model =
        env::var("OPENROUTER_MODEL")
            .unwrap_or_else(|_| {
                "openai/gpt-4o-mini"
                    .to_string()
            });

    let base_url =
        env::var("OPENROUTER_BASE_URL")
            .unwrap_or_else(|_| {
                "https://openrouter.ai/api/v1"
                    .to_string()
            });

    let client =
        OpenRouterClient::new(
            api_key,
            base_url,
        );

    let mut tools =
        ToolRegistry::new();

    tools.register(
        ShellTool::new()
    );
    tools.register(
        ProcessTool::new()
    );
    

    let mut agent =
        Agent::new(
            client,
            model,
            tools,
        );

    println!("Hyusk v0.1");
    println!(
        "Type 'exit' to quit.\n"
    );

    loop {
        print!("user › ");
        io::stdout().flush()?;

        let mut input =
            String::new();

        io::stdin()
            .read_line(&mut input)?;

        let input =
            input.trim();

        if input.is_empty() {
            continue;
        }

        if input == "exit" {
            break;
        }

        let response =
            agent
                .handle(
                    input.to_string()
                )
                .await?;

        println!(
            "\nHyusk: {}\n",
            response
        );
    }

    Ok(())
}