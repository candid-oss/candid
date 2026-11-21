//! Offline pinyin for the characters that fill most Chinese names (surnames and common given-name characters).
//!
//! A small table instead of a pinyin crate: it only has to recognise "zhangsan" / "sanzhang" / "zhangs" in an email
//! prefix, and an unknown character simply means "no corroboration" (the model's verdict stays a candidate).

const TABLE: &str = "张zhang李li王wang刘liu陈chen杨yang黄huang赵zhao吴wu周zhou徐xu孙sun马ma朱zhu胡hu郭guo何he高gao林lin罗luo郑zheng梁liang谢xie宋song唐tang许xu韩han冯feng邓deng曹cao彭peng曾zeng萧xiao田tian董dong袁yuan潘pan于yu蒋jiang蔡cai余yu杜du叶ye程cheng苏su魏wei吕lv丁ding任ren沈shen姚yao卢lu姜jiang崔cui钟zhong谭tan陆lu汪wang范fan金jin石shi廖liao贾jia夏xia韦wei付fu方fang白bai邹zou孟meng熊xiong秦qin邱qiu江jiang尹yin薛xue闫yan段duan雷lei侯hou龙long史shi陶tao黎li贺he顾gu毛mao郝hao龚gong邵shao万wan钱qian严yan覃qin武wu戴dai莫mo孔kong向xiang汤tang常chang温wen康kang施shi文wen牛niu樊fan葛ge邢xing安an齐qi易yi乔qiao伍wu庞pang颜yan倪ni庄zhuang聂nie章zhang鲁lu岳yue翟zhai殷yin詹zhan申shen欧ou耿geng关guan兰lan焦jiao俞yu左zuo柳liu甘gan祝zhu包bao宁ning尚shang符fu舒shu阮ruan柯ke纪ji梅mei童tong凌ling毕bi单shan季ji裴pei霍huo涂tu成cheng苗miao谷gu盛sheng曲qu翁weng冉ran骆luo蓝lan路lu游you辛xin靳jin管guan柴chai蒙meng鲍bao华hua喻yu祁qi蒲pu房fang滕teng屈qu饶rao解xie牟mou艾ai尤you阳yang时shi穆mu农nong司si卞bian班ban
三san四si五wu六liu七qi八ba九jiu十shi一yi二er小xiao大da老lao明ming伟wei芳fang娜na敏min静jing丽li强qiang磊lei军jun洋yang勇yong艳yan杰jie娟juan涛tao超chao秀xiu霞xia平ping刚gang桂gui英ying华hua玲ling燕yan红hong梅mei鹏peng辉hui建jian国guo志zhi文wen东dong海hai亮liang晨chen宇yu浩hao然ran凯kai博bo飞fei阳yang云yun帆fan俊jun雪xue婷ting莉li琳lin欣xin怡yi佳jia慧hui晶jing颖ying倩qian蕾lei萍ping兵bing斌bin彬bin龙long波bo宏hong鑫xin鹤he立li荣rong新xin天tian子zi思si梓zi轩xuan泽ze睿rui诗shi嘉jia若ruo安an宁ning乐le可ke依yi语yu雨yu萱xuan悦yue彤tong馨xin瑶yao琪qi茜xi菲fei薇wei蓉rong妍yan洁jie春chun冬dong夏xia秋qiu元yuan金jin玉yu山shan水shui木mu火huo土tu光guang德de成cheng功gong业ye林lin江jiang河he湖hu川chuan松song柏bai竹zhu兰lan菊ju荷he桐tong枫feng楠nan翔xiang翰han昊hao杰jie楷kai铭ming锋feng凡fan永yong康kang福fu禄lu寿shou喜xi富fu贵gui顺shun利li和he平ping忠zhong孝xiao仁ren义yi礼li智zhi信xin诚cheng良liang恒heng启qi哲zhe毅yi豪hao骏jun琦qi瑞rui承cheng希xi月yue星xing宝bao珍zhen珠zhu琴qin书shu画hua";

fn table() -> &'static std::collections::HashMap<char, &'static str> {
    static MAP: std::sync::OnceLock<std::collections::HashMap<char, &'static str>> = std::sync::OnceLock::new();
    MAP.get_or_init(|| {
        let mut m = std::collections::HashMap::new();
        let mut rest = TABLE;
        while let Some(c) = rest.chars().find(|c| !c.is_ascii() && !c.is_whitespace()) {
            let start = rest.find(c).unwrap() + c.len_utf8();
            let tail = &rest[start..];
            let end = tail.find(|c: char| !c.is_ascii_lowercase()).unwrap_or(tail.len());
            m.entry(c).or_insert(&tail[..end]);
            rest = &tail[end..];
        }
        m
    })
}

/// Syllables of a name of 2–4 Chinese characters, or `None` when any character is unknown or not Chinese.
fn syllables(name: &str) -> Option<Vec<&'static str>> {
    let chars: Vec<char> = name.chars().filter(|c| !c.is_whitespace()).collect();
    if !(2..=4).contains(&chars.len()) {
        return None;
    }
    chars.iter().map(|c| table().get(c).copied()).collect()
}

/// Spellings an email prefix could use for a Chinese name: `zhangsan`, `sanzhang`, `zhangs`, `szhang`, `zs`…
/// The family name is the first character. Forms shorter than four letters are dropped (too ambiguous).
pub(crate) fn email_forms(name: &str) -> Vec<String> {
    let Some(s) = syllables(name) else { return vec![] };
    let (family, given) = (s[0], &s[1..]);
    let given_all: String = given.concat();
    let given_initials: String = given.iter().filter_map(|g| g.chars().next()).collect();
    let family_initial = family.chars().next().unwrap_or(' ');
    let forms = [
        format!("{family}{given_all}"),
        format!("{given_all}{family}"),
        format!("{family}{given_initials}"),
        format!("{given_initials}{family}"),
        format!("{family_initial}{given_all}"),
        format!("{given_all}{family_initial}"),
    ];
    let mut out: Vec<String> = forms.into_iter().filter(|f| f.len() >= 4).collect();
    out.sort();
    out.dedup();
    out
}

/// Spellings for a Latin name such as "San Zhang": given+family in either order, with the given name in full or as
/// an initial.
pub(crate) fn latin_forms(name: &str) -> Vec<String> {
    let parts: Vec<String> = name.split(|c: char| !c.is_ascii_alphabetic()).filter(|p| !p.is_empty()).map(str::to_lowercase).collect();
    if parts.len() < 2 || !name.is_ascii() {
        return vec![];
    }
    let (first, last) = (&parts[0], &parts[parts.len() - 1]);
    let fi = &first[..1];
    let mut out: Vec<String> = [format!("{first}{last}"), format!("{last}{first}"), format!("{fi}{last}"), format!("{last}{fi}")].into_iter().filter(|f| f.len() >= 4).collect();
    out.sort();
    out.dedup();
    out
}

/// The part of an address before `@`, letters only (digits and separators dropped).
pub(crate) fn email_prefix(addr: &str) -> String {
    addr.split('@').next().unwrap_or("").to_lowercase().chars().filter(|c| c.is_ascii_lowercase()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_and_latin_names_meet_in_the_email_prefix() {
        assert!(email_forms("张三").contains(&"zhangsan".to_string()));
        assert!(email_forms("张三").contains(&"sanzhang".to_string()));
        assert!(email_forms("王小明").contains(&"wangxm".to_string()));
        assert!(email_forms("某某").is_empty());
        assert!(latin_forms("San Zhang").contains(&"szhang".to_string()));
        assert_eq!(email_prefix("San.Zhang88@x.com"), "sanzhang");
    }
}
