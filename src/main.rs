resource_model_macro::resource_model_file!("specs/self.yaml");

#[tokio::main]
async fn main() -> Result<(), sqlx::Error> {
    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = sqlx::PgPool::connect(&db_url).await?;

    migrate(&pool).await?;
    println!("✓ migrations applied");

    let orgs = SqlxOrganizationRepository::new(pool.clone());
    let users = SqlxUserRepository::new(pool.clone());
    let projects = SqlxProjectRepository::new(pool.clone());
    let tasks = SqlxTaskRepository::new(pool.clone());
    let entries = SqlxTimeEntryRepository::new(pool.clone());

    // ── Organization ───────────────────────────────────────
    let slug = format!("acme-{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let org = orgs
        .create(CreateOrganization {
            name: "Acme Corp".into(),
            slug,
            active: true,
        })
        .await?;
    println!("✓ org:     {} ({})", org.name, org.slug);

    // ── User (string, int, bool, optional text, FK→org) ───
    let email = format!("alice+{}@acme.io", &uuid::Uuid::new_v4().to_string()[..8]);
    let alice = users
        .create(CreateUser {
            name: "Alice".into(),
            email,
            age: 30,
            active: true,
            bio: Some("Engineer & coffee enthusiast".into()),
            org_id: org.id,
        })
        .await?;
    println!("✓ user:    {} (age={}, bio={:?})", alice.name, alice.age, alice.bio);

    // ── Project (float budget, bool, two FKs) ─────────────
    let proj = projects
        .create(CreateProject {
            name: "Launch v2".into(),
            description: None,
            budget: 50_000.0,
            active: true,
            org_id: org.id,
            owner_id: alice.id,
        })
        .await?;
    println!("✓ project: {} (budget={})", proj.name, proj.budget);

    // ── Task (int priority, bool done, optional float) ────
    let task = tasks
        .create(CreateTask {
            title: "Design landing page".into(),
            description: Some("Hero section + CTA".into()),
            priority: 1,
            done: false,
            estimated_hours: Some(8.5),
            project_id: proj.id,
            assignee_id: alice.id,
        })
        .await?;
    println!(
        "✓ task:    {} (priority={}, est={:?}h)",
        task.title, task.priority, task.estimated_hours
    );

    // ── TimeEntry (float hours, bool billable, optional bigint) ─
    let entry = entries
        .create(CreateTimeEntry {
            description: Some("Initial wireframes".into()),
            hours: 2.5,
            billable: true,
            rate_cents: Some(15000),
            task_id: task.id,
            user_id: alice.id,
        })
        .await?;
    println!(
        "✓ entry:   {:.1}h billable={} rate={:?}¢",
        entry.hours, entry.billable, entry.rate_cents
    );

    // ── Read ───────────────────────────────────────────────
    let found = tasks.find_by_id(task.id).await?;
    println!("\nfind_by_id(task): {found:?}");

    let all_orgs = orgs.list().await?;
    println!("list(orgs): {} org(s)", all_orgs.len());

    // ── Update (partial) ───────────────────────────────────
    let updated = tasks
        .update(
            task.id,
            UpdateTask {
                done: Some(true),
                estimated_hours: Some(6.0),
                ..Default::default()
            },
        )
        .await?;
    println!("updated task: {updated:?}");

    // ── Relations: has_many ────────────────────────────────
    let org_members = orgs.members(org.id).await?;
    println!("\norg.members: {}", org_members.len());

    let org_projects = orgs.projects(org.id).await?;
    println!("org.projects: {}", org_projects.len());

    let alice_projects = users.owned_projects(alice.id).await?;
    println!("alice.owned_projects: {}", alice_projects.len());

    let alice_tasks = users.assigned_tasks(alice.id).await?;
    println!("alice.assigned_tasks: {}", alice_tasks.len());

    let alice_entries = users.time_entries(alice.id).await?;
    println!("alice.time_entries: {}", alice_entries.len());

    let proj_tasks = projects.tasks(proj.id).await?;
    println!("project.tasks: {}", proj_tasks.len());

    let task_entries = tasks.entries(task.id).await?;
    println!("task.entries: {}", task_entries.len());

    // ── Relations: belongs_to ──────────────────────────────
    let alice_org = users.organization(alice.org_id).await?;
    println!("\nalice.organization: {:?}", alice_org.map(|o| o.name));

    let proj_owner = projects.owner(proj.owner_id).await?;
    println!("project.owner: {:?}", proj_owner.map(|u| u.name));

    let task_project = tasks.project(task.project_id).await?;
    println!("task.project: {:?}", task_project.map(|p| p.name));

    let task_assignee = tasks.assignee(task.assignee_id).await?;
    println!("task.assignee: {:?}", task_assignee.map(|u| u.name));

    let entry_task = entries.task(entry.task_id).await?;
    println!("entry.task: {:?}", entry_task.map(|t| t.title));

    let entry_worker = entries.worker(entry.user_id).await?;
    println!("entry.worker: {:?}", entry_worker.map(|u| u.name));

    // ── Cleanup (reverse dependency order) ─────────────────
    entries.delete(entry.id).await?;
    tasks.delete(task.id).await?;
    projects.delete(proj.id).await?;
    users.delete(alice.id).await?;
    orgs.delete(org.id).await?;
    println!("\n✓ all records cleaned up");

    Ok(())
}
