use std::{
    array,
    borrow::Borrow,
    cell::{Ref, RefCell},
    fmt::Result,
    fs::{self, File},
    io::Read,
    io::{self, Write},
    time::Duration,
};

use html_editor::{
    operation::{Queryable, Selector},
    parse, Element, Node,
};
// use html_query_parser::{parse, Element, Queryable, Selector};
use reqwest::{self, header::HeaderMap, Client};
use serde::de::value;
use sqlite::Connection;
#[macro_use]
extern crate serde_derive;

extern crate serde;
extern crate serde_json;

#[macro_use]
extern crate log;

const BASE_URL: &str = "http://www.tanpaifang.com/";

struct AppState {
    client: Client,
    connection: RefCell<Connection>,
}

impl<'a> AppState {
    fn from(client: Client, connection: RefCell<Connection>) -> Self {
        AppState { client, connection }
    }
    fn client(&self) -> Client {
        self.client.clone()
    }
    fn conn(&self) -> Ref<Connection> {
        self.connection.borrow()
    }
}

#[tokio::main]
async fn main() {
    env_logger::init();
    info!("starting up");
    let clinet = new_client();
    //创建数据连接
    let connection: RefCell<Connection> = match create_connction("./app.db").await {
        Ok(conn) => conn,
        Err(e) => {
            panic!("数据库连接创建失败!,{}", &e)
        }
    };
    let app_state = RefCell::new(AppState::from(clinet.clone(), connection));
    let app_state_clone = app_state.borrow();
    let conn = app_state_clone.conn();
    let mut cursor = conn
        .prepare("SELECT count(*) FROM navs")
        .unwrap()
        .into_cursor();
    let mut count = 0;
    while let Some(row) = cursor.next().unwrap() {
        count = row[0].as_integer().unwrap();
        if count > 0 {
            break;
        }
    }
    // println!("count==>{}", count);
    if count == 0 {
        //解析导航栏内容
        let navs = match navbox(clinet.clone()).await {
            Some(nav) => nav,
            None => {
                panic!("导航栏解析错误!")
            }
        };
        //导航写入数据库
        nav_insert_db(app_state.borrow(), &navs);
        //尝试解析数据链接
        parse_nav_link(&navs, app_state.borrow()).await;
    } else {
        let navs = select_navs(app_state.borrow()).unwrap();
        //尝试解析数据链接
        parse_nav_link(&navs, app_state.borrow()).await;
    }
}

//尝试解析导航页面地址信息
async fn parse_nav_link(navs: &Vec<Nav>, app_state: Ref<'_, AppState>) {
    println!("{:?}", navs);
    for nav in navs.iter() {
        if nav.state == 0 {
            let mut link = nav.path.clone();
            link.remove(0);
            let sql  = format!("CREATE TABLE if not exists  {}_news (herf TEXT, title TEXT,info TEXT,copyright TEXT,created_at TEXT);",link);
            println!("{:?}", sql);
            app_state.borrow().connection.borrow().execute(sql).unwrap();
            let client = app_state.client();
            let req = client.get(format!("{}{}", BASE_URL, nav.path));
            let resp = match req.send().await {
                Ok(resp) => resp,
                Err(_) => return,
            };
            //解析数据
            let html_text = match resp.text().await {
                Ok(html) => html,
                Err(_) => {
                    return;
                }
            };
            //解析html界面
            let html = match parse(&html_text) {
                Ok(h) => h,
                Err(_) => {
                    return;
                }
            };

            //解析获取页面节点数据
            let selector = Selector::from(".left_list_box");
            let mut news_list = Vec::<News>::new();
            html.query_all(&selector).iter().for_each(|f| {
                //(Element { name, attrs, children }
                if let Some(news) = _parse_element(&f) {
                    news_list.push(news);
                }
            });
            //获取所有的第一页的数据
            let selector = Selector::from(".pag_1");
            let element = html.query_all(&selector);
            //尝试解析下一页的内容
            for ele in element {
                for ele in ele.children.iter() {
                    if ele.is_element() {
                        let ele_clone = ele.clone();
                        let node = ele_clone.into_element();
                        let app_state = app_state.borrow();
                        //解析获取下一页数据
                        parse_next_btn(app_state, &node, &link).await;
                    }
                }
            }
        }
    }
}
//解析下一页数据
async fn parse_next_btn(app_state: &Ref<'_, AppState>, f: &Element, link: &String) {
    println!("<====+++++++++++++++++++++++++++++++++++++++++++++====>");
    // println!("========>{:?}",f);
    let mut  page_list = f.children.query_all(&Selector::from(".ntub"));
    let page_size_strong = f.children.query_all(&Selector::from("strong"));
    let mut page_size = 0;
    let mut news_size = 0;
    //获取页面数量
    page_size_strong.iter().enumerate().for_each(|(i, v)| {
        if i == 0 {
            for ele in v.children.iter() {
                match ele {
                    Node::Text(s) => {
                        page_size = s.parse().unwrap();
                        break;
                    }
                    _ => {}
                }
            }
        } else {
            for ele in v.children.iter() {
                match ele {
                    Node::Text(s) => {
                        news_size = s.parse().unwrap();
                        break;
                    }
                    _ => {}
                }
            }
        }
    });
    //获取下一页界面

    let mut count = 2;
    if page_size != 0 {
        while page_size != count {
            //获取下一页
            let mut next_href = String::from("");
            for ele in page_list.iter() {
                let next = ele.query(&Selector::from("a")).unwrap();
                for (k, (i, v)) in next.attrs.iter().enumerate() {
                    if i.eq("href") {
                        next_href = v.to_string();
                        break;
                    }
                }
            }
            let path = format!("{}{}/{}", BASE_URL, link, next_href);
            let app_state = &*app_state.borrow();
            let next_page = next_page_text(app_state,&path).await.unwrap();

            let news = get_news_list(&next_page).await;
            match news {
                Some(news) => {
                    println!(
                        "解析第{}页,一共{},剩余{}未解析",
                        count,
                        page_size,
                        page_size - count
                    );
                    _insert_to_sqlite_db(app_state.conn(), news, link).await;
                }
                None => {
                    println!(
                        "解析第{}页出现错误,一共{},剩余{}未解析",
                        count,
                        page_size,
                        page_size - count
                    )
                }
            }
            //解析下一页按钮
            page_list = f.children.query_all(&Selector::from(".ntub"));
            

            count += 1;
        }
    }
    println!("<====+++++++++++++++++++++++++++++++++++++++++++++====>");
}


async fn next_page_text(app_state: &Ref<'_, AppState>,herf: &str)->Option<Vec<Node>>{
    let client = app_state.client();
    let request_build = client.get(herf);
    //获取请求结果
    let response_result = match request_build.send().await {
        Ok(resp) => resp,
        Err(e) => {
            error!("请求页面数据错误");
            return None;
        }
    };
    info!("start parse html text:{:?}", &response_result);
    //html
    let html_text = match response_result.text().await {
        Ok(v) => v,
        Err(e) => {
            error!("html_text html_text:{:?}", e);
            return None;
        }
    };

    //解析Html
    let html = match parse(html_text.as_str()) {
        Ok(h) => h,
        Err(_) => return None,
    };
    Some(html)
}


//查询数据库中数据
fn select_navs(app_state: Ref<'_, AppState>) -> Option<Vec<Nav>> {
    //解析获取页面数据
    let conn = app_state.conn();
    let mut cursor = conn.prepare("SELECT * FROM navs").unwrap().into_cursor();
    let mut navs: Vec<Nav> = Vec::new();
    while let Some(row) = cursor.next().unwrap() {
        let mut nav = Nav::default();
        nav.path = row[1].as_string().unwrap().to_string();
        nav.text = row[2].as_string().unwrap().to_string();
        navs.push(nav);
    }
    Some(navs)
}

//创建数据库
async fn create_connction(db: &str) -> io::Result<RefCell<Connection>> {
    let connection: RefCell<Connection>;
    if fs::File::open("co.lock").is_ok() {
        connection = RefCell::new(sqlite::open("./app.db").unwrap());
    } else {
        connection = RefCell::new(sqlite::open("./app.db").unwrap());
        if let Err(e) = connection.borrow().execute("CREATE TABLE navs (_id INTEGER PRIMARY KEY AUTOINCREMENT,link TEXT, title text,state INTEGER DEFAULT 0);",
        ) {
            panic!("数据库初始化错误{}", e);
        }
        //创建锁
        let _ = File::create("co.lock").unwrap();
    }

    Ok(connection)
}
fn gen_default_headers() -> HeaderMap {
    let mut header = HeaderMap::new();
    header.insert("User-Agent", 
    r#"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)  AppleWebKit/537.36 (KHTML, like Gecko) Chrome/100.0.4896.127 Safari/537.36"#.parse().unwrap());
    // header.insert("key", "val".parse().unwrap());
    header
}

async fn get_news_list(html:&Vec<Node>) -> Option<Vec<News>> {
    let selector = Selector::from(".left_list_box");
    let mut news_list = Vec::<News>::new();
    html.query_all(&selector).iter().for_each(|f| {
        //(Element { name, attrs, children }
        if let Some(news) = _parse_element(&f) {
            news_list.push(news);
        }
    });
    return Some(news_list);
}

fn _parse_element(f: &Element) -> Option<News> {
    let mut nes = News::default();

    let _title = match f.children.query(&Selector::from(".title")) {
        Some(v) => v,
        None => return None,
    };
    let title = _title.attrs;
    for (k, v) in title {
        if k.eq("href") {
            nes.herf = v.to_string();
        }
        if k.eq("title") {
            nes.herf = v.to_string();
        }
    }
    let _banquan = match f.children.query(&Selector::from(".banquan")) {
        Some(v) => v,
        None => return None,
    };
    for (index, value) in _banquan.children.iter().enumerate() {
        if value.is_element() {
            let f1 = value.clone();
            let ele = f1.into_element();
            let texts: Vec<String> = ele
                .children
                .iter()
                .map(|e| -> String {
                    match e {
                        Node::Text(s) => return s.to_string(),
                        _ => return String::from(""),
                    }
                })
                .collect();
            if index == 0 {
                nes.copyright = match texts.get(0) {
                    Some(s) => s.to_string(),
                    None => return None,
                }
            } else {
                nes.created_at = match texts.get(0) {
                    Some(s) => s.to_string(),
                    None => return None,
                }
            }
        }
    }
    let _miaoshu = match f.children.query(&Selector::from(".miaoshu")) {
        Some(v) => v,
        None => return None,
    };
    for ele in _miaoshu.children.iter() {
        match ele {
            Node::Text(s) => {
                nes.info = s.clone();
            }
            _ => {}
        }
    }
    return Some(nes);
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct News {
    herf: String,
    title: String,
    info: String,
    copyright: String,
    created_at: String,
    article: String,
}
impl Default for News {
    fn default() -> Self {
        Self {
            herf: Default::default(),
            title: Default::default(),
            info: Default::default(),
            copyright: Default::default(),
            created_at: Default::default(),
            article: Default::default(),
        }
    }
}

//保存数据到文件
fn _write_to_file(news: Vec<News>, page: i32) {
    let mut jfile = match fs::File::create(format!("./_data/new_{}.json", page)) {
        Ok(f) => f,
        Err(e) => match e.kind() {
            std::io::ErrorKind::NotFound => {
                fs::create_dir("./_data").unwrap();
                fs::File::create(format!("./_data/new_{}.json", page)).unwrap()
            }
            _ => return,
        },
    };
    let json_text = serde_json::to_string(&news).unwrap();
    jfile.write(&json_text.as_bytes()).unwrap();
}
//咨询信息保存数据库
async fn _insert_to_sqlite_db(conn: Ref<'_, Connection>, news: Vec<News>, table_name: &str) {
    for ele in news.iter() {
        let sql = format!(
            "INSERT INTO {}_news(herf,title,info,copyright,created_at)VALUES('{}','{}','{}','{}','{}')",
            table_name,ele.herf, ele.title, ele.info, ele.copyright, ele.created_at
        );
        println!("{}", &sql);
        let result = conn.execute(sql);
        if let Ok(()) = result {
            println!("成功插入一条数据")
        }
        println!("change_count:==>{}", conn.change_count())
    }
}

/// new_client 新建网络请求
fn new_client() -> Client {
    let client = {
        let default_headers = gen_default_headers();
        reqwest::Client::builder()
            .default_headers(default_headers.clone())
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap()
    };
    client
}

async fn navbox(clinet: Client) -> Option<Vec<Nav>> {
    let http_build = clinet.get("http://www.tanpaifang.com/").send().await;
    let response = match http_build {
        Ok(resp) => resp,
        Err(_) => return None,
    };
    let html_text = response.text().await;
    //解析文本内容
    let html_text_parse = match html_text {
        Ok(h) => h,
        Err(_) => return None,
    };
    let html = match parse(&html_text_parse) {
        Ok(h) => h,
        Err(_) => return None,
    };
    let selector = Selector::from(".navbox");
    let mut navs_list = Vec::<Nav>::new();
    html.query_all(&selector).iter().for_each(|f| {
        if let Some(mut navs) = parse_nav_head(&f) {
            navs_list.append(&mut navs)
        }
    });
    Some(navs_list)
}

fn parse_nav_head(f: &Element) -> Option<Vec<Nav>> {
    let mut nav_list: Vec<Nav> = Vec::new();
    for (_, value) in f.children.iter().enumerate() {
        let node = value.clone();
        if node.is_element() {
            let element = node.into_element();
            let nvas = element.query_all(&Selector::from("a"));
            nvas.iter().for_each(|v| {
                let mut nav = Nav::new("".to_string(), "".to_string());
                v.attrs.iter().for_each(|(k, v)| {
                    if k.eq("href") {
                        nav.path = v.to_string();
                    }
                });
                v.children.iter().for_each(|v| {
                    if let Node::Text(s) = v {
                        nav.text = s.to_string();
                    }
                });

                nav_list.push(nav);
            });
        }
    }
    Some(nav_list)
}

fn nav_insert_db(app_state: Ref<AppState>, v: &Vec<Nav>) {
    for ele in v.iter() {
        let sql = format!(
            "INSERT INTO navs (link,title)VALUES('{}','{}')",
            ele.path, ele.text
        );
        println!("{}", &sql);
        let result = app_state.borrow().connection.borrow().execute(sql);
        if let Ok(()) = result {
            println!(
                "change_count:==>{}",
                app_state.borrow().connection.borrow().change_count()
            )
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct Nav {
    path: String,
    text: String,
    href: String,
    state: i32,
}

impl Nav {
    fn new(path: String, text: String) -> Self {
        Nav {
            path,
            text,
            state: 0,
            href: "".to_string(),
        }
    }
}

impl Default for Nav {
    fn default() -> Self {
        Self {
            path: Default::default(),
            text: Default::default(),
            state: Default::default(),
            href: Default::default(),
        }
    }
}
