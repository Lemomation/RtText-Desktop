#![allow(dead_code)]

pub struct Config {
    pub supabase_url: &'static str,
    pub supabase_anon_key: &'static str,
    pub default_email: &'static str,
    pub default_password: &'static str,
}

pub const CONFIG: Config = Config {
    supabase_url: "https://jskzgedrciwklcmmpwcx.supabase.co",
    supabase_anon_key: "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6Impza3pnZWRyY2l3a2xjbW1wd2N4Iiwicm9sZSI6ImFub24iLCJpYXQiOjE3OTEyNjQ0NTUsImV4cCI6MjEwNjg0MDQ1NX0.dGuJgQlt1IgpEgc_2FcHA2HfAM75spuYaI4Oftc7L2I",
    default_email: "uabidalt@gmail.com",
    default_password: "RtTextDesktop2026!",
};
