use std::{
    collections::{HashMap, HashSet, hash_map},
    error::Error,
    fs::File,
    io::{Read, Write},
    path::PathBuf,
    process::Command,
    str::MatchIndices,
};

use clap::Parser as _;
use serde::{Deserialize, Serialize, de::Visitor};
use serde_derive::{Deserialize, Serialize};

#[derive(Deserialize, Debug)]
struct ZfsListOutputVersion {
    command: String,
    vers_major: i32,
    vers_minor: i32,
}

#[derive(Deserialize, Debug)]
enum DatasetType {
    #[serde(rename(deserialize = "FILESYSTEM"))]
    FileSystem,
    #[serde(rename(deserialize = "SNAPSHOT"))]
    Snapshot,
    #[serde(rename(deserialize = "ZVOL"))]
    Zvol,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PropertyValue {
    Integer(i64),
    String(String),
}

impl PropertyValue {
    pub fn to_string(&self) -> String {
        match self {
            PropertyValue::Integer(int) => int.to_string(),
            PropertyValue::String(string) => string.clone(),
        }
    }

    pub fn new_string<S>(str: S) -> PropertyValue
    where
        S: AsRef<str>,
    {
        PropertyValue::String(str.as_ref().to_owned())
    }

    pub fn new_integer(integer: i64) -> PropertyValue {
        PropertyValue::Integer(integer)
    }
}

struct PropertyValueVisitor;

impl<'de> Visitor<'de> for PropertyValueVisitor {
    type Value = PropertyValue;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("either a integer or string")
    }

    fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(Self::Value::Integer(v))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(Self::Value::Integer(v as i64))
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(Self::Value::String(v.to_owned()))
    }
}

impl<'de> Deserialize<'de> for PropertyValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(PropertyValueVisitor)
    }
}

impl Serialize for PropertyValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            PropertyValue::Integer(int) => int.serialize(serializer),
            PropertyValue::String(str) => str.serialize(serializer),
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
enum PropertySource {
    #[serde(rename(deserialize = "LOCAL"))]
    Local { data: String },
    #[serde(rename(deserialize = "NONE"))]
    None { data: String },
    #[serde(rename(deserialize = "INHERITED"))]
    Inherited { data: String },
    #[serde(rename(deserialize = "DEFAULT"))]
    Default { data: String },
    #[serde(rename(deserialize = "TEMPORARY"))]
    TEMPORARY { data: String },
}

#[derive(Deserialize, Debug)]
struct Property {
    value: PropertyValue,
    source: PropertySource,
}

#[derive(Deserialize, Debug)]
struct Dataset {
    name: String,
    r#type: DatasetType,
    pool: String,
    createtxg: i32,
    properties: HashMap<String, Property>,
}

impl Dataset {
    pub fn get_property<'a, S>(&'a self, name: S) -> Option<&'a Property>
    where
        S: AsRef<str>,
    {
        self.properties.get(name.as_ref())
    }

    pub fn get_property_mut<'a, S>(&'a mut self, name: S) -> Option<&'a mut Property>
    where
        S: AsRef<str>,
    {
        self.properties.get_mut(name.as_ref())
    }
}

#[derive(Deserialize, Debug)]
struct ZfsListOutput {
    output_version: ZfsListOutputVersion,
    datasets: HashMap<String, Dataset>,
}

impl ZfsListOutput {
    pub fn from_command<I, S>(command: Option<I>) -> Result<ZfsListOutput, std::io::Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        fn go<'a, I>(head: &str, tail: I) -> Result<ZfsListOutput, std::io::Error>
        where
            I: Iterator<Item = &'a str>,
        {
            Command::new(head)
                .args(tail)
                .output()
                .and_then(|output| serde_json::from_slice(&output.stdout).map_err(|err| err.into()))
        }

        match command {
            Some(command) => {
                let iter1 = command.into_iter().collect::<Vec<S>>();
                let mut iter = iter1.iter().map(|s| s.as_ref());
                go(iter.next().unwrap(), iter)
            }
            None => go("zfs", ["get", "all", "--json", "--json-int"].into_iter()),
        }
    }

    pub fn from_reader<R>(rdr: R) -> Result<ZfsListOutput, serde_json::Error>
    where
        R: Read,
    {
        serde_json::from_reader(rdr)
    }

    pub fn get_dataset<'a, S>(&'a self, name: S) -> Option<&'a Dataset>
    where
        S: AsRef<str>,
    {
        self.datasets.get(name.as_ref())
    }

    pub fn get_dataset_mut<'a, S>(&'a mut self, name: S) -> Option<&'a mut Dataset>
    where
        S: AsRef<str>,
    {
        self.datasets.get_mut(name.as_ref())
    }
}

#[derive(Debug)]
enum ZfsAction {
    CreateDataset {
        name: String,
        properties: HashMap<String, PropertyValue>,
    },
    SetProperties {
        dataset: String,
        properties: HashMap<String, PropertyValue>,
    },
}

#[derive(Deserialize, Serialize)]
struct ZfsSpecDataset {
    name: String,
    properties: HashMap<String, PropertyValue>,
}

impl ZfsSpecDataset {
    pub fn new<S1, S2>(name: S1, properties: HashMap<S2, PropertyValue>) -> ZfsSpecDataset
    where
        S1: AsRef<str>,
        S2: AsRef<str>,
    {
        ZfsSpecDataset {
            name: name.as_ref().to_owned(),
            properties: properties
                .into_iter()
                .map(|(k, v)| (k.as_ref().to_owned(), v))
                .collect(),
        }
    }
}

#[derive(Deserialize, Serialize)]
struct ZfsSpec {
    datasets: Vec<ZfsSpecDataset>,
}

impl ZfsSpec {
    pub fn from_reader<R>(rdr: R) -> Result<ZfsSpec, serde_json::Error>
    where
        R: Read,
    {
        serde_json::from_reader(rdr)
    }
}

#[derive(Debug)]
struct ActionSet(Vec<ZfsAction>);

impl ActionSet {
    pub fn to_commands(&self) -> Vec<Vec<String>> {
        self.0
            .iter()
            .map(|action| match action {
                ZfsAction::CreateDataset { name, properties } => {
                    let mut output = Vec::with_capacity(3 + properties.len());
                    output.extend_from_slice(&["zfs", "create"].map(|x| x.to_owned()));
                    output.extend(
                        properties
                            .iter()
                            .map(|(name, value)| format!("-o {}={}", name, value.to_string())),
                    );
                    output.push(name.to_owned());
                    output
                }
                ZfsAction::SetProperties {
                    dataset,
                    properties,
                } => {
                    vec![
                        "zfs".to_owned(),
                        "set".to_owned(),
                        properties
                            .into_iter()
                            .map(|(name, value)| format!("{}={}", name, value.to_string()))
                            .collect::<Vec<_>>()
                            .join(" "),
                        dataset.to_owned(),
                    ]
                }
            })
            .collect()
    }
}

#[derive(Debug)]
struct VecActionProducer {
    actions: Vec<ZfsAction>,
    errors: Vec<String>,
}

impl VecActionProducer {
    fn new() -> VecActionProducer {
        VecActionProducer {
            actions: Vec::new(),
            errors: Vec::new(),
        }
    }

    fn cleanup_multiple_creates(actions: Vec<ZfsAction>) -> Vec<ZfsAction> {
        let mut known_datasets: HashMap<String, HashMap<String, PropertyValue>> = HashMap::new();

        actions
            .into_iter()
            .flat_map::<Box<[ZfsAction]>, _>(|action| match &action {
                ZfsAction::CreateDataset { name, properties } => {
                    log::trace!("optimizing {:?}", action);
                    let mut edited_properties = HashMap::new();

                    if let Some(existing_properties) = known_datasets.get(name) {
                        log::trace!("known dateset {}", name);
                        for (name, value) in properties {
                            if let Some(existing_value) = existing_properties.get(name) {
                                if existing_value != value {
                                    log::trace!(
                                        "existing property {} {} != {}",
                                        name,
                                        existing_value.to_string(),
                                        value.to_string()
                                    );
                                    edited_properties.insert(name.clone(), value.clone());
                                } else {
                                    log::trace!(
                                        "existing property {} {} == {}",
                                        name,
                                        existing_value.to_string(),
                                        value.to_string()
                                    );
                                }
                            } else {
                                log::trace!("adding property {}={}", name, value.to_string());
                                edited_properties.insert(name.clone(), value.clone());
                            }
                        }
                        for (property_name, property_value) in &edited_properties {
                            log::trace!(
                                "dataset {} setting {}={}",
                                name,
                                property_name,
                                property_value.to_string()
                            )
                        }

                        edited_properties
                            .into_iter()
                            .map(|(property_name, property_value)| ZfsAction::SetProperties {
                                dataset: name.clone(),
                                properties: HashMap::from([(property_name, property_value)]),
                            })
                            .collect()
                    } else {
                        log::trace!("new dateset {}, keeping", name);
                        known_datasets.insert(name.clone(), properties.clone());
                        [action].into_iter().collect()
                    }
                }
                ZfsAction::SetProperties { .. } => [action].into_iter().collect(),
            })
            .collect::<Vec<_>>()
    }

    fn cleanup_shadowed_properties(actions: Vec<ZfsAction>) -> Vec<ZfsAction> {
        let mut set_properties = HashSet::new();

        actions
            .into_iter()
            .rev()
            .filter_map(|action| match action {
                ZfsAction::CreateDataset {
                    name: dataset,
                    properties,
                } => Some(ZfsAction::CreateDataset {
                    properties: properties
                        .into_iter()
                        .filter(|(name, value)| {
                            if set_properties.contains(&(dataset.clone(), name.clone())) {
                                log::trace!(
                                    "dataset {} discard shadowed property {}={}",
                                    dataset,
                                    name,
                                    value.to_string()
                                );
                                false
                            } else {
                                log::trace!(
                                    "dataset {} keep new property {}={}",
                                    dataset,
                                    name,
                                    value.to_string()
                                );
                                set_properties.insert((dataset.clone(), name.clone()));
                                true
                            }
                        })
                        .collect(),
                    name: dataset,
                }),
                ZfsAction::SetProperties {
                    dataset,
                    properties,
                } => Some(ZfsAction::SetProperties {
                    properties: properties
                        .into_iter()
                        .filter_map(|(name, value)| {
                            if set_properties.contains(&(dataset.clone(), name.clone())) {
                                log::trace!(
                                    "dataset {} shadowed property {}={}",
                                    dataset,
                                    name,
                                    value.to_string()
                                );
                                None
                            } else {
                                log::trace!("dataset {} new property {}", dataset, name);
                                set_properties.insert((dataset.clone(), name.clone()));
                                Some((name, value))
                            }
                        })
                        .collect(),
                    dataset,
                }),
            })
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    fn cleanup_fold_properties_into_creates(actions: Vec<ZfsAction>) -> Vec<ZfsAction> {
        let mut creates: HashMap<String, HashMap<String, PropertyValue>> = actions
            .iter()
            .filter_map(|action| match action {
                ZfsAction::CreateDataset { name, properties } => {
                    Some((name.to_owned(), properties.to_owned()))
                }
                ZfsAction::SetProperties {
                    dataset,
                    properties,
                } => None,
            })
            .collect();

        actions
            .into_iter()
            .filter_map(|action| match action {
                ZfsAction::CreateDataset { .. } => Some(action),
                ZfsAction::SetProperties {
                    dataset,
                    properties,
                } => {
                    if creates.contains_key(&dataset) {
                        creates.entry(dataset).and_modify(|existing_properties| {
                            existing_properties.extend(properties)
                        });

                        None
                    } else {
                        Some(ZfsAction::SetProperties {
                            dataset,
                            properties,
                        })
                    }
                }
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|action| match action {
                ZfsAction::CreateDataset {
                    name,
                    properties: _properties,
                } => ZfsAction::CreateDataset {
                    properties: creates.remove(&name).unwrap(),
                    name,
                },
                ZfsAction::SetProperties {
                    dataset,
                    properties,
                } => ZfsAction::SetProperties {
                    dataset,
                    properties,
                },
            })
            .collect()
    }

    fn cleanup(&mut self) {
        self.actions = vec![
            Self::cleanup_multiple_creates,
            Self::cleanup_shadowed_properties,
            Self::cleanup_fold_properties_into_creates,
        ]
        .into_iter()
        .fold(std::mem::take(&mut self.actions), |acc, f| f(acc))
    }

    fn finalize(mut self) -> (ActionSet, Vec<String>) {
        // self.cleanup();
        (ActionSet(self.actions), self.errors)
    }
}

impl ActionProducer for VecActionProducer {
    fn produce_action(&mut self, action: ZfsAction) {
        self.actions.push(action)
    }

    fn produce_error(&mut self, error: String) {
        self.errors.push(error)
    }
}

trait ActionProducer {
    fn produce_action(&mut self, action: ZfsAction);
    fn produce_error(&mut self, error: String);
}

fn eval_spec<AP>(action_producer: &mut AP, state: &ZfsListOutput, spec: &ZfsSpec)
where
    AP: ActionProducer,
{
    for dataset in spec.datasets.iter() {
        if let Some(dataset_state) = state.get_dataset(&dataset.name) {
            log::trace!("dataset {} already exists", dataset.name);

            let mut properties = HashMap::new();

            for (property, value) in &dataset.properties {
                if let Some(property_state) = dataset_state.get_property(property) {
                    match property_state.source {
                        PropertySource::Local { .. }
                        | PropertySource::Inherited { .. }
                        | PropertySource::Default { .. } => {
                            if property_state.value != *value {
                                log::trace!(
                                    "dataset {} property {} set to {}, modify to {}",
                                    dataset.name,
                                    property,
                                    property_state.value.to_string(),
                                    value.to_string()
                                );
                                properties.insert(property.to_owned(), value.to_owned());
                            } else {
                                log::trace!(
                                    "dataset {} property {} already set to {}, skip",
                                    dataset.name,
                                    property,
                                    value.to_string()
                                );
                            }
                        }
                        _ => {
                            log::trace!(
                                "dataset {} property {} not normal, error",
                                dataset.name,
                                property,
                            );
                            action_producer.produce_error(format!(
                                "cannot set property {} of dataset {} because source is {:?}",
                                property, dataset.name, property_state.source
                            ))
                        }
                    }
                } else {
                    log::trace!(
                        "dataset {} property {} not set, set to {}",
                        dataset.name,
                        property,
                        value.to_string(),
                    );
                    properties.insert(property.to_owned(), value.to_owned());
                }
            }

            action_producer.produce_action(ZfsAction::SetProperties {
                dataset: dataset.name.to_owned(),
                properties,
            })
        } else {
            log::trace!("prepare dataset {}", dataset.name);

            struct PrefixPaths<'a>(&'a str, MatchIndices<'a, char>);

            impl<'a> PrefixPaths<'a> {
                fn new(string: &'a str) -> PrefixPaths<'a> {
                    PrefixPaths(string, string.match_indices('/'))
                }
            }

            impl<'a> Iterator for PrefixPaths<'a> {
                type Item = &'a str;

                fn next(&mut self) -> Option<Self::Item> {
                    self.1.next().map(|(index, _)| &self.0[0..index])
                }
            }

            for dataset_part in PrefixPaths::new(&dataset.name) {
                if state.get_dataset(dataset_part).is_none() {
                    log::trace!("create parent dataset {}", dataset_part);
                    action_producer.produce_action(ZfsAction::CreateDataset {
                        name: dataset_part.to_owned(),
                        properties: HashMap::new(),
                    })
                }
            }
            log::trace!(
                "create dataset {} with properties {}",
                dataset.name,
                dataset
                    .properties
                    .iter()
                    .map(|(name, value)| format!("{}={}", name, value.to_string()))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            action_producer.produce_action(ZfsAction::CreateDataset {
                name: dataset.name.to_owned(),
                properties: dataset.properties.to_owned(),
            })
        }
    }
}

#[derive(clap::Args, Clone)]
#[clap(group(
    clap::ArgGroup::new("source")
    .required(true)
    .args(&["command", "file"])))]
struct Source {
    #[clap(short, long)]
    command: bool,
    #[clap(short, long)]
    file: Option<PathBuf>,
}

#[derive(clap::Parser)]
#[command(name = "disko-zfs")]
struct Cli {
    #[arg(short, long)]
    spec: PathBuf,
    #[clap(flatten)]
    source: Source,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Clone, clap::Subcommand)]
enum Commands {
    Plan {
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    Apply,
}

fn main() -> Result<(), Box<dyn Error>> {
    simple_logger::SimpleLogger::new().env().init().unwrap();

    let cli = Cli::parse();

    // let zfs_spec = ZfsSpec {
    //     datasets: vec![
    //         ZfsSpecDataset::new(
    //             "zroot/test",
    //             HashMap::from(
    //                 [
    //                     (":test", PropertyValue::new_string("test")),
    //                     ("recordsize", PropertyValue::new_integer(8192)),
    //                 ]
    //                 .map(|(k, v)| (k.to_owned(), v)),
    //             ),
    //         ),
    //         ZfsSpecDataset::new(
    //             "zroot/test",
    //             HashMap::from(
    //                 [
    //                     (":test", PropertyValue::new_string("test")),
    //                     ("recordsize", PropertyValue::new_integer(16384)),
    //                 ]
    //                 .map(|(k, v)| (k.to_owned(), v)),
    //             ),
    //         ),
    //         ZfsSpecDataset::new(
    //             "zroot/ds1/persist/var/lib/postgresql",
    //             HashMap::from(
    //                 [
    //                     (":test", PropertyValue::new_string("test")),
    //                     ("recordsize", PropertyValue::new_integer(16384)),
    //                     ("compressratio", PropertyValue::new_string("2.0")),
    //                 ]
    //                 .map(|(k, v)| (k.to_owned(), v)),
    //             ),
    //         ),
    //         ZfsSpecDataset::new("zroot/test/test", HashMap::<String, _>::new()),
    //     ],
    // };
    // serde_json::to_writer_pretty(std::io::stdout(), &zfs_spec)?;

    let zfs_list_output: ZfsListOutput = if cli.source.command {
        ZfsListOutput::from_command::<Vec<_>, String>(None)?
    } else if let Some(file) = cli.source.file {
        let file = File::open(file)?;
        ZfsListOutput::from_reader(file)?
    } else {
        unreachable!()
    };

    let zfs_spec = {
        let file = File::open(cli.spec)?;
        ZfsSpec::from_reader(file)?
    };

    match cli.command {
        Commands::Plan { output } => {
            let mut ap = VecActionProducer::new();

            eval_spec(&mut ap, &zfs_list_output, &zfs_spec);

            let commands = ap.finalize().0.to_commands();

            if let Some(output) = output {
                let mut output = File::create(output)?;

                for command in commands {
                    write!(&mut output, "{}\n", command.join(" "))?;
                }
            } else {
                for command in commands {
                    println!("> {}", command.join(" "))
                }
            }

            Ok(())
        }
        Commands::Apply => todo!(),
    }
}
