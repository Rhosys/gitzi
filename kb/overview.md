# gitzi Overview

gitzi is a Kanban agent harness for software development pipelines. It orchestrates AI coding agents across a development lifecycle with human-in-the-loop approval.

## Core Concepts

- **Epic**: A top-level unit of work containing one or more tasks
- **Task**: A single unit of work assigned to an agent, tracked through pipeline stages
- **Fork**: A branched conversation for a different thought while the main thread continues
- **Board**: Kanban view showing tasks across pipeline stages
- **Review Item**: A decision point requiring human input

## Pipeline Stages

Tasks flow through: Prioritized → Designing → Coding → Reviewing → Testing → Auditing → Deploying → Done

Each stage has a buffer column where tasks wait for human approval before advancing.

## Chat Interface

All interaction happens through the chat on the left side. The main agent manages the project through natural conversation — creating epics, tasks, answering questions, and driving the review queue.

The right side shows system state (panels: Status, Epic, Kanban, Task, Logs).
